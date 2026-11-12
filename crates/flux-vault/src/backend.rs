//! Where repository files live. Local directories now; SFTP implements the same trait.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flux_core::sync::Locked;
use flux_transport::lock::{self as locks, LocalStore, LockInfo, LockStore};

/// A held repository lock; released on drop.
pub struct LockGuard {
    _inner: Box<dyn Send + Sync>,
}

impl LockGuard {
    pub fn new(inner: impl Send + Sync + 'static) -> Self {
        Self {
            _inner: Box::new(inner),
        }
    }
}

/// One read of [`Backend::read_many`]: path, offset, and length (`None`: the whole file).
pub type ReadAt = (String, u64, Option<u64>);

pub trait Backend: Send + Sync {
    /// Human-readable location, for messages.
    fn describe(&self) -> String;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    fn read_range(&self, path: &str, offset: u64, len: u64) -> io::Result<Vec<u8>>;
    /// Many reads at once, answered in the order asked: `(path, offset, length)`, the
    /// length `None` for the whole file. On a server this costs a couple of round trips
    /// for a batch of files instead of several for each (snapshot records, index files,
    /// pack headers: hundreds or thousands of small reads).
    fn read_many(&self, reads: &[ReadAt]) -> Vec<io::Result<Vec<u8>>> {
        use rayon::prelude::*;
        reads
            .par_iter()
            .map(|(path, offset, len)| match len {
                None => self.read(path),
                Some(len) => self.read_range(path, *offset, *len),
            })
            .collect()
    }
    /// Is every read a round trip to another computer? There, a pack that more than one
    /// blob is wanted from is fetched whole, once (see `VaultRepo::read_blob`).
    fn remote(&self) -> bool {
        false
    }
    /// Create or replace `path` atomically (readers see the old or the new content), and
    /// make it durable before returning.
    fn write(&self, path: &str, data: &[u8]) -> io::Result<()>;
    /// Like [`Backend::write`], but durability may wait for [`Backend::flush_packs`]. For
    /// pack files, which nothing refers to until an index is written: a backup writes all
    /// its packs, flushes once, then writes the index. A crash before that leaves packs no
    /// index names, which the next prune removes.
    fn write_pack(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.write(path, data)
    }
    /// Make every [`Backend::write_pack`] so far durable.
    fn flush_packs(&self) -> io::Result<()> {
        Ok(())
    }
    /// How many packs a backup writes at once, in the background (see
    /// `Packer::write_behind`). A server: several uploads in flight.
    fn pack_writers(&self) -> usize {
        4
    }
    /// Files directly in `dir`, or (for `data` and `parity/data`) in its two-hex-digit
    /// subdirectories:
    /// `(name, size)`. A missing directory is empty.
    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>>;
    fn remove(&self, path: &str) -> io::Result<()>;
    fn exists(&self, path: &str) -> io::Result<bool>;
    fn lock(&self, exclusive: bool) -> io::Result<Option<LockGuard>>;
    /// Who holds the repository's lock files (for "in use by …" messages).
    fn lock_holders(&self) -> io::Result<Vec<LockInfo>>;
    /// Remove stale lock files, or (`force`) every lock file. Returns those removed.
    fn clear_locks(&self, force: bool) -> io::Result<Vec<LockInfo>>;
}

thread_local! {
    /// Each pack writer's aligned buffer for [`LocalBackend::write_direct`].
    static DIRECT_BUF: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
}

/// Filesystems of local drives, where `O_DIRECT` writes go straight to the device. Others
/// (network filesystems, FUSE, tmpfs) write as they always did: there, `O_DIRECT` can
/// mean a round trip per write, or isn't supported at all.
fn direct_io_suits(root: &Path) -> bool {
    const LOCAL: [i64; 7] = [
        0xEF53,      // ext2/3/4
        0x5846_5342, // xfs
        0x9123_683E, // btrfs
        0xF2F5_2010, // f2fs
        0x2011_BAB0, // exfat
        0x4D44,      // vfat
        0x7366_746E, // ntfs3
    ];
    // (`f_type` is narrower than `i64` on 32-bit systems.)
    #[allow(clippy::unnecessary_cast)]
    rustix::fs::statfs(root).is_ok_and(|s| LOCAL.contains(&(s.f_type as i64)))
}

/// A repository in a local (or locally mounted) directory.
#[derive(Debug, Clone)]
pub struct LocalBackend {
    root: PathBuf,
    /// Whether packs are written with `O_DIRECT` (see [`LocalBackend::write_direct`]):
    /// decided at the first pack, and turned off for good if the kernel refuses one.
    direct: Arc<std::sync::OnceLock<std::sync::atomic::AtomicBool>>,
}

impl LocalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            direct: Arc::default(),
        }
    }

    /// Write packs straight to the drive? On the filesystems of local drives, unless
    /// `FLUX_PACK_WRITES=buffered`.
    fn direct(&self) -> &std::sync::atomic::AtomicBool {
        self.direct.get_or_init(|| {
            let wanted = std::env::var_os("FLUX_PACK_WRITES").is_none_or(|v| v != "buffered");
            std::sync::atomic::AtomicBool::new(wanted && direct_io_suits(&self.root))
        })
    }

    /// A pack written with `O_DIRECT`, past the page cache: the drive takes it as fast as
    /// it can, and a backup doesn't fill memory with gigabytes of data waiting to be
    /// written (Linux lets a fifth to two fifths of the RAM wait; meanwhile a backup looks
    /// faster than the drive, then waits at the end, and other programs are short of
    /// memory). The data is copied into an aligned buffer (whole 4 KiB blocks, the tail
    /// zeroed) and the file then cut to its length. Durable at [`Backend::flush_packs`], as
    /// a buffered pack is.
    fn write_direct(&self, path: &str, data: &[u8]) -> io::Result<()> {
        const ALIGN: usize = 4096;
        let dest = self.path(path);
        let dir = dest.parent().expect("repository files live in directories");
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{:x}",
            std::process::id(),
            u64::from_le_bytes(crate::crypto::random().map_err(io::Error::other)?)
        ));
        let whole = data.len().div_ceil(ALIGN) * ALIGN;
        let mut buf = DIRECT_BUF.take();
        buf.clear();
        buf.resize(whole + ALIGN, 0);
        let at = buf.as_ptr().align_offset(ALIGN);
        let block = &mut buf[at..at + whole];
        block[..data.len()].copy_from_slice(data);
        block[data.len()..].fill(0);
        let result = (|| {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::DIRECT.bits() as i32)
                .open(&tmp)?;
            f.write_all(block)?;
            if whole != data.len() {
                f.set_len(data.len() as u64)?;
            }
            fs::rename(&tmp, &dest)
        })();
        if buf.capacity() <= crate::pack::KEEP_BUFFER {
            DIRECT_BUF.set(buf);
        }
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, rel: &str) -> PathBuf {
        debug_assert!(!rel.contains(".."), "relative repository path {rel:?}");
        self.root.join(rel)
    }

    /// Write via a temporary file and a rename (readers see the old or the new content);
    /// `durable`: fsync the file and its directory before returning.
    fn write_file(&self, path: &str, data: &[u8], durable: bool) -> io::Result<()> {
        let dest = self.path(path);
        let dir = dest.parent().expect("repository files live in directories");
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{:x}",
            std::process::id(),
            u64::from_le_bytes(crate::crypto::random().map_err(io::Error::other)?)
        ));
        let result = (|| {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(data)?;
            if durable {
                f.sync_all()?;
            }
            fs::rename(&tmp, &dest)?;
            if durable {
                File::open(dir)?.sync_all()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

impl Backend for LocalBackend {
    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        fs::read(self.path(path))
    }

    fn read_range(&self, path: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let mut f = File::open(self.path(path))?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.write_file(path, data, true)
    }

    /// No fsync of the pack and its directory (two per 16 MiB pack: on a USB hard drive,
    /// minutes over a large first backup); one `syncfs` in [`Backend::flush_packs`].
    ///
    /// Written past the page cache where the filesystem allows (see `write_direct`).
    fn write_pack(&self, path: &str, data: &[u8]) -> io::Result<()> {
        use std::sync::atomic::Ordering::Relaxed;
        let direct = self.direct();
        if direct.load(Relaxed) {
            match self.write_direct(path, data) {
                // Not on this filesystem (or this file's blocks): buffered from now on.
                Err(e) if e.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error()) => {
                    direct.store(false, Relaxed);
                }
                r => return r,
            }
        }
        self.write_file(path, data, false)
    }

    fn pack_writers(&self) -> usize {
        2
    }

    fn flush_packs(&self) -> io::Result<()> {
        let root = File::open(&self.root)?;
        rustix::fs::syncfs(&root).map_err(io::Error::from)
    }

    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        let base = self.path(dir);
        let rd = match fs::read_dir(&base) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for e in rd {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue; // temporaries
            }
            let ft = e.file_type()?;
            if ft.is_dir() && fanned_out(dir) {
                for f in fs::read_dir(e.path())? {
                    let f = f?;
                    let n = f.file_name().to_string_lossy().into_owned();
                    if !n.starts_with('.') && f.file_type()?.is_file() {
                        out.push((n, f.metadata()?.len()));
                    }
                }
            } else if ft.is_file() {
                out.push((name, e.metadata()?.len()));
            }
        }
        out.sort();
        Ok(out)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        match fs::remove_file(self.path(path)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    fn exists(&self, path: &str) -> io::Result<bool> {
        self.path(path).try_exists()
    }

    /// `flock` on `lock` (what Flux before 1.0 used, and what releases itself if the
    /// process dies), plus a lock file in `locks/`, the same files a client reaching this
    /// repository over SFTP writes and respects.
    fn lock(&self, exclusive: bool) -> io::Result<Option<LockGuard>> {
        fs::create_dir_all(&self.root)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.path("lock"))?;
        let op = if exclusive {
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        } else {
            rustix::fs::FlockOperation::NonBlockingLockShared
        };
        match rustix::fs::flock(&f, op) {
            Ok(()) => {}
            Err(rustix::io::Errno::WOULDBLOCK) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let store: Arc<dyn LockStore> = Arc::new(LocalStore);
        Ok(locks::acquire_in(store, &self.locks_dir(), exclusive)?
            .ok()
            .map(|file| LockGuard::new((f, file))))
    }

    fn lock_holders(&self) -> io::Result<Vec<LockInfo>> {
        locks::holders(&LocalStore, &self.locks_dir())
    }

    fn clear_locks(&self, force: bool) -> io::Result<Vec<LockInfo>> {
        locks::clear(&LocalStore, &self.locks_dir(), force)
    }
}

impl LocalBackend {
    fn locks_dir(&self) -> String {
        self.path("locks").display().to_string()
    }
}

/// A repository on an SSH server, over SFTP.
pub struct SftpBackend {
    pool: std::sync::Arc<flux_transport::SftpPool>,
    root: String,
    limiter: Option<Limiter>,
}

/// The upload limit now, in bytes per second (`None`: unlimited). Asked as a backup runs,
/// so a limit by time of day changes mid-backup.
pub type RateNow = Box<dyn Fn() -> Option<u64> + Send + Sync>;

/// Bandwidth limiter for uploads: sleeps to keep the average rate under the limit. When
/// the limit changes, the average starts again from that moment.
struct Limiter {
    rate: RateNow,
    /// (since, bytes since then, the rate they were measured against)
    state: std::sync::Mutex<(std::time::Instant, u64, Option<u64>)>,
}

impl Limiter {
    fn wait(&self, bytes: u64) {
        let rate = (self.rate)().filter(|r| *r > 0);
        let delay = {
            let mut st = self.state.locked();
            if st.2 != rate {
                *st = (std::time::Instant::now(), 0, rate);
            }
            let Some(r) = rate else { return };
            st.1 += bytes;
            let should_take = std::time::Duration::from_secs_f64(st.1 as f64 / r as f64);
            should_take.saturating_sub(st.0.elapsed())
        };
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }
}

impl SftpBackend {
    /// `root` is the repository path on the server (relative paths are relative to the
    /// remote home directory).
    pub fn new(
        pool: std::sync::Arc<flux_transport::SftpPool>,
        root: String,
        bandwidth_limit: Option<u64>,
    ) -> Self {
        let limiter = bandwidth_limit.filter(|b| *b > 0).map(|b| Limiter {
            rate: Box::new(move || Some(b)),
            state: std::sync::Mutex::new((std::time::Instant::now(), 0, Some(b))),
        });
        Self {
            pool,
            root: root.trim_end_matches('/').to_owned(),
            limiter,
        }
    }

    /// Limit uploads by `rate`, asked as the backup runs (a limit by time of day).
    pub fn with_rate(mut self, rate: RateNow) -> Self {
        self.limiter = Some(Limiter {
            rate,
            state: std::sync::Mutex::new((std::time::Instant::now(), 0, None)),
        });
        self
    }

    fn path(&self, rel: &str) -> String {
        debug_assert!(!rel.contains(".."));
        format!("{}/{rel}", self.root)
    }
}

impl Backend for SftpBackend {
    fn describe(&self) -> String {
        format!(
            "ssh://{}/{}",
            self.pool.target().destination(),
            self.root.trim_start_matches('/')
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.pool.with(|s| s.read_file(&self.path(path)))
    }

    fn read_range(&self, path: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len as usize);
        self.pool.with(|s| {
            s.read_range(&self.path(path), offset, len, |c| {
                out.extend_from_slice(c);
                Ok(())
            })
        })?;
        if out.len() as u64 != len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{path}: short read"),
            ));
        }
        Ok(out)
    }

    fn read_many(&self, reads: &[ReadAt]) -> Vec<io::Result<Vec<u8>>> {
        use flux_transport::sftp::ReadReq;
        use rayon::prelude::*;
        // A batch to each session the pool has free.
        let reqs: Vec<ReadReq> = reads
            .iter()
            .map(|(path, offset, len)| ReadReq {
                path: self.path(path).into_bytes(),
                offset: *offset,
                len: *len,
            })
            .collect();
        let mut out: Vec<io::Result<Vec<u8>>> = reqs
            .par_chunks(32)
            .flat_map_iter(|chunk| match self.pool.with(|s| Ok(s.read_many(chunk))) {
                Ok(v) => v,
                Err(e) => chunk
                    .iter()
                    .map(|_| Err(io::Error::new(e.kind(), e.to_string())))
                    .collect(),
            })
            .collect();
        // A range must come whole, as `read_range` gives it.
        for (r, (path, _, len)) in out.iter_mut().zip(reads) {
            if let (Ok(data), Some(len)) = (&r, len)
                && data.len() as u64 != *len
            {
                *r = Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{path}: short read"),
                ));
            }
        }
        out
    }

    fn remote(&self) -> bool {
        true
    }

    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        if let Some(l) = &self.limiter {
            l.wait(data.len() as u64);
        }
        let dest = self.path(path);
        let dir = dest.rsplit_once('/').map_or(".", |(d, _)| d).to_owned();
        let mut r = [0u8; 8];
        getrandom::fill(&mut r).map_err(|e| io::Error::other(e.to_string()))?;
        let tmp = format!("{dir}/.tmp-{}", hex::encode(r));
        self.pool.with(|s| {
            match s.write_new(&tmp, data, 0o600) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    s.mkdir_p(&dir, 0o700)?;
                    s.write_new(&tmp, data, 0o600)?;
                }
                other => other?,
            }
            s.rename(&tmp, &dest).inspect_err(|_| {
                let _ = s.remove(&tmp);
            })
        })
    }

    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        let base = self.path(dir);
        let entries = match self.pool.with(|s| s.readdir(&base)) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for (name, attrs) in entries {
            if name.starts_with('.') {
                continue;
            }
            if attrs.is_dir() && fanned_out(dir) {
                for (n, a) in self.pool.with(|s| s.readdir(&format!("{base}/{name}")))? {
                    if !n.starts_with('.') && a.is_file() {
                        out.push((n, a.size.unwrap_or(0)));
                    }
                }
            } else if attrs.is_file() {
                out.push((name, attrs.size.unwrap_or(0)));
            }
        }
        out.sort();
        Ok(out)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        match self.pool.with(|s| s.remove(&self.path(path))) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    fn exists(&self, path: &str) -> io::Result<bool> {
        match self.pool.with(|s| s.lstat(&self.path(path))) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn lock(&self, exclusive: bool) -> io::Result<Option<LockGuard>> {
        Ok(
            flux_transport::lock::acquire(self.pool.clone(), &self.path("locks"), exclusive)?
                .map(LockGuard::new),
        )
    }

    fn lock_holders(&self) -> io::Result<Vec<LockInfo>> {
        locks::holders(self.pool.as_ref(), &self.path("locks"))
    }

    fn clear_locks(&self, force: bool) -> io::Result<Vec<LockInfo>> {
        locks::clear(self.pool.as_ref(), &self.path("locks"), force)
    }
}

/// Directories whose files sit in two-hex-digit subdirectories: the packs, and their
/// parity files.
fn fanned_out(dir: &str) -> bool {
    dir == "data" || dir == "parity/data"
}

/// Path of a pack inside the repository.
pub fn pack_path(id_hex: &str) -> String {
    format!("data/{}/{id_hex}", &id_hex[..2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_backend_basics() {
        let t = tempfile::tempdir().unwrap();
        let b = LocalBackend::new(t.path());
        b.write("snapshots/a", b"hello").unwrap();
        b.write(&pack_path("abcdef"), b"0123456789").unwrap();
        assert_eq!(b.read("snapshots/a").unwrap(), b"hello");
        assert_eq!(b.read_range(&pack_path("abcdef"), 2, 3).unwrap(), b"234");
        assert_eq!(b.list("data").unwrap(), [("abcdef".to_string(), 10)]);
        assert_eq!(b.list("snapshots").unwrap(), [("a".to_string(), 5)]);
        assert!(b.list("index").unwrap().is_empty());
        b.remove("snapshots/a").unwrap();
        b.remove("snapshots/a").unwrap();
        assert!(!b.exists("snapshots/a").unwrap());

        let shared1 = b.lock(false).unwrap().unwrap();
        let shared2 = b.lock(false).unwrap().unwrap();
        assert!(
            b.lock(true).unwrap().is_none(),
            "exclusive waits for shared holders"
        );
        drop((shared1, shared2));
        let ex = b.lock(true).unwrap().unwrap();
        assert!(b.lock(false).unwrap().is_none());
        assert_eq!(b.lock_holders().unwrap().len(), 1, "the lock file names it");
        drop(ex);
        assert!(
            b.lock_holders().unwrap().is_empty(),
            "released with the lock"
        );
    }

    /// A job reaching the same Vault over SFTP (another computer, here the local SFTP
    /// stand-in) holds a lock file: the local backend respects it, and says who it is.
    #[test]
    fn local_backend_respects_sftp_locks() {
        let t = tempfile::tempdir().unwrap();
        let Some(target) = flux_transport::testing::local_target(t.path()) else {
            return;
        };
        let local = LocalBackend::new(t.path().join("vault"));
        let pool = Arc::new(flux_transport::SftpPool::new(target, 2));
        let remote = SftpBackend::new(pool, t.path().join("vault").display().to_string(), None);
        let r = remote.lock(true).unwrap().unwrap();
        assert!(local.lock(false).unwrap().is_none());
        assert_eq!(local.lock_holders().unwrap().len(), 1);
        drop(r);
        let l = local.lock(false).unwrap().unwrap();
        assert!(
            remote.lock(true).unwrap().is_none(),
            "and the other way round"
        );
        assert!(remote.lock(false).unwrap().is_some());
        drop(l);
    }
}
