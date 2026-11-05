//! Where repository files live. Local directories now; SFTP implements the same trait.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

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

pub trait Backend: Send + Sync {
    /// Human-readable location, for messages.
    fn describe(&self) -> String;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    fn read_range(&self, path: &str, offset: u64, len: u64) -> io::Result<Vec<u8>>;
    /// Create or replace `path` atomically (readers see the old or the new content).
    fn write(&self, path: &str, data: &[u8]) -> io::Result<()>;
    /// Files directly in `dir`, or (for `data`) in its two-hex-digit subdirectories:
    /// `(name, size)`. A missing directory is empty.
    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>>;
    fn remove(&self, path: &str) -> io::Result<()>;
    fn exists(&self, path: &str) -> io::Result<bool>;
    fn lock(&self, exclusive: bool) -> io::Result<Option<LockGuard>>;
}

/// A repository in a local (or locally mounted) directory.
#[derive(Debug, Clone)]
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, rel: &str) -> PathBuf {
        debug_assert!(!rel.contains(".."), "relative repository path {rel:?}");
        self.root.join(rel)
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
            f.sync_all()?;
            fs::rename(&tmp, &dest)?;
            File::open(dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
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
            if ft.is_dir() && dir == "data" {
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
            Ok(()) => Ok(Some(LockGuard {
                _inner: Box::new(f),
            })),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// A repository on an SSH server, over SFTP.
pub struct SftpBackend {
    pool: std::sync::Arc<flux_transport::SftpPool>,
    root: String,
    limiter: Option<Limiter>,
}

/// Simple bandwidth limiter for uploads: sleeps to keep the average rate under the limit.
struct Limiter {
    bytes_per_sec: u64,
    state: std::sync::Mutex<(std::time::Instant, u64)>,
}

impl Limiter {
    fn wait(&self, bytes: u64) {
        let delay = {
            let mut st = self.state.lock().expect("limiter lock");
            st.1 += bytes;
            let should_take =
                std::time::Duration::from_secs_f64(st.1 as f64 / self.bytes_per_sec as f64);
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
            bytes_per_sec: b,
            state: std::sync::Mutex::new((std::time::Instant::now(), 0)),
        });
        Self {
            pool,
            root: root.trim_end_matches('/').to_owned(),
            limiter,
        }
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
            if attrs.is_dir() && dir == "data" {
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
        drop(ex);
    }
}
