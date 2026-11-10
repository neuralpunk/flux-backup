//! Low-level filesystem operations: reading and applying metadata, copying with
//! reflink/copy_file_range, and removing read-only trees.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::BorrowedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, XattrFlags};
use rustix::io::Errno;

use flux_core::progress::Progress;

use crate::entry::{Entry, Kind, Xattr};

/// Which extended attributes to read and restore.
#[derive(Debug, Clone, Copy)]
pub struct XattrPolicy {
    pub acl: bool,
    pub other: bool,
}

impl XattrPolicy {
    pub fn any(self) -> bool {
        self.acl || self.other
    }

    fn wants(self, name: &[u8]) -> bool {
        if name.starts_with(b"system.posix_acl_") {
            self.acl
        } else {
            // security.* (SELinux labels, capabilities) is host policy, not user data; it is
            // relabelled by the target system and copying it often fails or is wrong.
            self.other && !name.starts_with(b"security.") && !name.starts_with(b"system.")
        }
    }
}

/// Read the extended attributes of `path` (not following symlinks), sorted by name.
pub fn read_xattrs(path: &Path, policy: XattrPolicy) -> io::Result<Vec<Xattr>> {
    if !policy.any() {
        return Ok(Vec::new());
    }
    let len = match rustix::fs::llistxattr_query(path) {
        Ok(0) => return Ok(Vec::new()),
        Ok(n) => n,
        Err(Errno::NOTSUP) | Err(Errno::PERM) => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut names = vec![0u8; len + 64];
    let n = match rustix::fs::llistxattr(path, &mut names[..]) {
        Ok(n) => n,
        Err(Errno::RANGE) => return read_xattrs(path, policy), // grew meanwhile; retry
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for name in names[..n].split(|&b| b == 0).filter(|s| !s.is_empty()) {
        if !policy.wants(name) {
            continue;
        }
        let vlen = match rustix::fs::lgetxattr_query(path, name) {
            Ok(v) => v,
            Err(Errno::NODATA) => continue,
            Err(e) => return Err(e.into()),
        };
        let mut value = vec![0u8; vlen];
        let got = match rustix::fs::lgetxattr(path, name, &mut value[..]) {
            Ok(g) => g,
            Err(Errno::NODATA) => continue,
            Err(e) => return Err(e.into()),
        };
        value.truncate(got);
        out.push(Xattr {
            name: name.to_vec(),
            value,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// What metadata may be applied to the destination.
#[derive(Debug)]
pub struct ApplyPolicy {
    /// We're root: set owner/group, create device nodes.
    pub privileged: bool,
    pub xattrs: XattrPolicy,
    /// Cleared after the first "not supported" so we stop trying (and stop warning).
    pub xattrs_supported: AtomicBool,
}

impl ApplyPolicy {
    pub fn new(xattrs: XattrPolicy) -> Self {
        Self {
            privileged: flux_core::sys::is_root(),
            xattrs,
            xattrs_supported: AtomicBool::new(true),
        }
    }
}

/// Apply ownership, xattrs, permissions and mtime from `e` to `dst`, in the order that
/// works: xattrs first (setting them can need write permission), then chown (which clears
/// setuid bits), then chmod, and the times last.
pub fn apply_metadata(dst: &Path, e: &Entry, pol: &ApplyPolicy) -> io::Result<()> {
    let is_link = e.kind == Kind::Symlink;
    if !is_link && !e.xattrs.is_empty() && pol.xattrs_supported.load(Relaxed) {
        for x in &e.xattrs {
            match rustix::fs::lsetxattr(dst, &x.name[..], &x.value, XattrFlags::empty()) {
                Ok(()) => {}
                Err(Errno::NOTSUP) => {
                    pol.xattrs_supported.store(false, Relaxed);
                    tracing::warn!("target filesystem does not support extended attributes");
                    break;
                }
                // Not permitted for this namespace as non-root: keep it in the manifest only.
                Err(Errno::PERM) | Err(Errno::ACCESS) => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    if pol.privileged {
        std::os::unix::fs::lchown(dst, Some(e.uid), Some(e.gid))?;
    }
    if !is_link {
        fs::set_permissions(dst, fs::Permissions::from_mode(e.mode & 0o7777))?;
    }
    set_mtime(dst, e.mtime, e.mtime_nsec)
}

fn mtime_only(sec: i64, nsec: u32) -> Timestamps {
    Timestamps {
        last_access: Timespec {
            tv_sec: 0,
            tv_nsec: rustix::fs::UTIME_OMIT,
        },
        last_modification: Timespec {
            tv_sec: sec,
            tv_nsec: nsec.into(),
        },
    }
}

/// [`apply_metadata`] through a descriptor of a regular file or directory that the caller
/// opened itself, so nothing can be swapped in between. `mode_mask` clears permission bits
/// (e.g. setuid) on top of the policy.
pub fn apply_metadata_fd(
    fd: BorrowedFd<'_>,
    e: &Entry,
    pol: &ApplyPolicy,
    mode_mask: u32,
) -> io::Result<()> {
    if !e.xattrs.is_empty() && pol.xattrs_supported.load(Relaxed) {
        for x in &e.xattrs {
            match rustix::fs::fsetxattr(fd, &x.name[..], &x.value, XattrFlags::empty()) {
                Ok(()) => {}
                Err(Errno::NOTSUP) => {
                    pol.xattrs_supported.store(false, Relaxed);
                    tracing::warn!("target filesystem does not support extended attributes");
                    break;
                }
                Err(Errno::PERM) | Err(Errno::ACCESS) => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    if pol.privileged {
        rustix::fs::fchown(
            fd,
            Some(rustix::fs::Uid::from_raw(e.uid)),
            Some(rustix::fs::Gid::from_raw(e.gid)),
        )?;
    }
    rustix::fs::fchmod(
        fd,
        rustix::fs::Mode::from_raw_mode(e.mode & 0o7777 & !mode_mask),
    )?;
    rustix::fs::futimens(fd, &mtime_only(e.mtime, e.mtime_nsec)).map_err(Into::into)
}

/// Ownership and mtime of the symlink `name` in `dir` (symlinks have no permissions).
pub fn apply_link_metadata_at(
    dir: BorrowedFd<'_>,
    name: &OsStr,
    e: &Entry,
    pol: &ApplyPolicy,
) -> io::Result<()> {
    if pol.privileged {
        rustix::fs::chownat(
            dir,
            name,
            Some(rustix::fs::Uid::from_raw(e.uid)),
            Some(rustix::fs::Gid::from_raw(e.gid)),
            AtFlags::SYMLINK_NOFOLLOW,
        )?;
    }
    rustix::fs::utimensat(
        dir,
        name,
        &mtime_only(e.mtime, e.mtime_nsec),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(Into::into)
}

/// Metadata of the FIFO or device node `name` in `dir`, which the caller just created.
/// It's opened with `O_PATH` (opening a FIFO for real would block) and checked to be a
/// special file before anything changes; extended attributes are not restored on these.
pub fn apply_special_metadata_at(
    dir: BorrowedFd<'_>,
    name: &OsStr,
    e: &Entry,
    pol: &ApplyPolicy,
    mode_mask: u32,
) -> io::Result<()> {
    use rustix::fs::{FileType, Mode, OFlags};
    let fd = rustix::fs::openat(
        dir,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let st = rustix::fs::fstat(&fd)?;
    if !matches!(
        FileType::from_raw_mode(st.st_mode),
        FileType::Fifo | FileType::CharacterDevice | FileType::BlockDevice
    ) {
        return Err(io::Error::other("was replaced while being restored"));
    }
    if pol.privileged {
        rustix::fs::chownat(
            &fd,
            "",
            Some(rustix::fs::Uid::from_raw(e.uid)),
            Some(rustix::fs::Gid::from_raw(e.gid)),
            AtFlags::EMPTY_PATH,
        )?;
    }
    // chmod has no descriptor form for O_PATH; /proc/self/fd/N reaches exactly this inode.
    let proc = crate::safe::proc_path(&fd, OsStr::new(""));
    rustix::fs::chmodat(
        CWD,
        &proc,
        Mode::from_raw_mode(e.mode & 0o7777 & !mode_mask),
        AtFlags::empty(),
    )?;
    rustix::fs::utimensat(
        dir,
        name,
        &mtime_only(e.mtime, e.mtime_nsec),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(Into::into)
}

/// Create a FIFO or device node `name` in `dir` (mode 0600 until metadata is applied).
pub fn make_special_at(dir: BorrowedFd<'_>, name: &OsStr, e: &Entry) -> io::Result<()> {
    use rustix::fs::{FileType, Mode};
    let ft = match e.kind {
        Kind::Fifo => FileType::Fifo,
        Kind::CharDevice => FileType::CharacterDevice,
        Kind::BlockDevice => FileType::BlockDevice,
        _ => return Ok(()),
    };
    rustix::fs::mknodat(dir, name, ft, Mode::from_raw_mode(0o600), e.rdev).map_err(Into::into)
}

/// Set the modification time without following symlinks; access time is left alone.
pub fn set_mtime(dst: &Path, sec: i64, nsec: u32) -> io::Result<()> {
    let times = Timestamps {
        last_access: Timespec {
            tv_sec: 0,
            tv_nsec: rustix::fs::UTIME_OMIT,
        },
        last_modification: Timespec {
            tv_sec: sec,
            tv_nsec: nsec.into(),
        },
    };
    rustix::fs::utimensat(CWD, dst, &times, AtFlags::SYMLINK_NOFOLLOW).map_err(Into::into)
}

/// Copy settings shared by every copy in one job.
#[derive(Debug)]
pub struct Copier {
    reflink: AtomicBool,
    copy_range: AtomicBool,
}

impl Default for Copier {
    fn default() -> Self {
        Self {
            reflink: AtomicBool::new(true),
            copy_range: AtomicBool::new(true),
        }
    }
}

const CHUNK: usize = 8 << 20;

impl Copier {
    /// Copy the contents of `src` to a new file `dst` (mode 0600 until metadata is applied).
    /// Tries a reflink first (instant on Btrfs/XFS when both sides share a filesystem), then
    /// in-kernel `copy_file_range`, then a userspace loop. Progress is updated per chunk.
    /// Returns the number of bytes copied.
    pub fn copy(&self, src: &Path, dst: &Path, progress: &Progress) -> io::Result<u64> {
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dst)?;
        self.copy_to(src, &output, progress)
    }

    /// Like [`Copier::copy`], into a file that is already open (and empty).
    pub fn copy_to(&self, src: &Path, output: &File, progress: &Progress) -> io::Result<u64> {
        self.copy_from(File::open(src)?, output, progress)
    }

    /// Like [`Copier::copy_to`], from file `rel` of a snapshot's stored tree at `root`,
    /// for a restore. A stored tree someone else can write to is not trusted: `rel` is
    /// opened below `root` without following any link (see [`open_below`]), so a link
    /// planted there can't make a restore copy a file from elsewhere.
    pub fn copy_below(
        &self,
        root: &Path,
        rel: &Path,
        output: &File,
        progress: &Progress,
    ) -> io::Result<u64> {
        self.copy_from(open_below(root, rel)?, output, progress)
    }

    /// Like [`Copier::copy_to`], from a file that is already open (a backup opens its
    /// sources with a [`SourceOpener`]).
    pub fn copy_from(
        &self,
        mut input: File,
        output: &File,
        progress: &Progress,
    ) -> io::Result<u64> {
        let mut output = output;
        let meta = input.metadata()?;
        let len = meta.len();

        if self.reflink.load(Relaxed) {
            match rustix::fs::ioctl_ficlone(output, &input) {
                Ok(()) => {
                    progress.bytes_done.fetch_add(len, Relaxed);
                    return Ok(len);
                }
                Err(_) => self.reflink.store(false, Relaxed),
            }
        }

        // Sparse files (disk images, databases) keep their holes: only the data is copied.
        if is_sparse(&meta)
            && let Some(n) = self.copy_sparse(&input, output, len, progress)?
        {
            return Ok(n);
        }

        let mut copied = 0u64;
        if self.copy_range.load(Relaxed) {
            loop {
                if progress.is_cancelled() {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                match rustix::fs::copy_file_range(&input, None, output, None, CHUNK) {
                    Ok(0) => return Ok(copied),
                    Ok(n) => {
                        copied += n as u64;
                        progress.bytes_done.fetch_add(n as u64, Relaxed);
                    }
                    Err(Errno::XDEV | Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP)
                        if copied == 0 =>
                    {
                        self.copy_range.store(false, Relaxed);
                        break;
                    }
                    Err(Errno::INTR) => continue,
                    Err(e) => return Err(e.into()),
                }
            }
        }

        let mut buf = vec![0u8; 1 << 20];
        loop {
            if progress.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            output.write_all(&buf[..n])?;
            copied += n as u64;
            progress.bytes_done.fetch_add(n as u64, Relaxed);
        }
        Ok(copied)
    }
}

/// Fewer blocks allocated than the size needs: the file has holes (or the filesystem
/// compresses it; then the whole file is data and nothing is lost by looking).
pub fn is_sparse(m: &fs::Metadata) -> bool {
    m.len() > 0 && m.blocks().saturating_mul(512) < m.len()
}

/// Where the data of a sparse file lies, as (offset, length) runs in order, up to `len`;
/// everything else is holes. `None` when the file has no holes worth recording (it isn't
/// sparse, or is all data) or the filesystem can't say (`SEEK_DATA` unsupported).
pub fn data_segments(file: &File, len: u64) -> io::Result<Option<Vec<(u64, u64)>>> {
    use rustix::fs::{SeekFrom, seek};
    if !is_sparse(&file.metadata()?) {
        return Ok(None);
    }
    let mut runs = Vec::new();
    let mut pos = 0u64;
    while pos < len {
        let start = match seek(file, SeekFrom::Data(pos)) {
            Ok(s) => s,
            // No data after `pos`: the rest is a hole.
            Err(Errno::NXIO) => break,
            Err(Errno::INVAL | Errno::OPNOTSUPP) if pos == 0 => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if start >= len {
            break;
        }
        let end = seek(file, SeekFrom::Hole(start))?.min(len);
        runs.push((start, end - start));
        pos = end;
    }
    Ok((runs.as_slice() != [(0, len)]).then_some(runs))
}

impl Copier {
    /// Copy only the data regions of `input` (`SEEK_DATA`/`SEEK_HOLE`), then set the
    /// length so a trailing hole stays a hole. `None` if the filesystem can't say where the
    /// data is. Returns the data bytes copied; progress counts the whole length.
    fn copy_sparse(
        &self,
        input: &File,
        output: &File,
        len: u64,
        progress: &Progress,
    ) -> io::Result<Option<u64>> {
        use rustix::fs::{SeekFrom, seek};
        let mut pos = 0u64;
        let mut data_bytes = 0u64;
        while pos < len {
            let start = match seek(input, SeekFrom::Data(pos)) {
                Ok(s) => s,
                // No data after `pos`: the rest is a hole.
                Err(Errno::NXIO) => break,
                Err(Errno::INVAL | Errno::OPNOTSUPP) if pos == 0 => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            let end = seek(input, SeekFrom::Hole(start))?.min(len);
            self.copy_segment(input, output, start, end, progress)?;
            data_bytes += end - start;
            pos = end;
        }
        output.set_len(len)?;
        progress.bytes_done.fetch_add(len - data_bytes, Relaxed);
        Ok(Some(data_bytes))
    }

    /// Copy bytes `start..end` of `input` to the same offsets of `output`.
    fn copy_segment(
        &self,
        input: &File,
        output: &File,
        start: u64,
        end: u64,
        progress: &Progress,
    ) -> io::Result<()> {
        use std::os::unix::fs::FileExt;
        let (mut off_in, mut off_out) = (start, start);
        while off_in < end {
            if progress.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let want = ((end - off_in) as usize).min(CHUNK);
            let n = if self.copy_range.load(Relaxed) {
                match rustix::fs::copy_file_range(
                    input,
                    Some(&mut off_in),
                    output,
                    Some(&mut off_out),
                    want,
                ) {
                    Ok(0) => break, // the file shrank under us
                    Ok(n) => n,
                    Err(Errno::INTR) => continue,
                    Err(Errno::XDEV | Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => {
                        self.copy_range.store(false, Relaxed);
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                let mut buf = vec![0u8; want.min(1 << 20)];
                let n = match input.read_at(&mut buf, off_in) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                output.write_all_at(&buf[..n], off_out)?;
                off_in += n as u64;
                off_out += n as u64;
                n
            };
            progress.bytes_done.fetch_add(n as u64, Relaxed);
        }
        Ok(())
    }
}

/// BLAKE3 of a file's contents.
pub fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    hash_open(File::open(path)?)
}

/// BLAKE3 of an open file's contents.
pub fn hash_open(file: File) -> io::Result<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    h.update_reader(file)?;
    Ok(*h.finalize().as_bytes())
}

/// Opens the files a backup reads, as the scan saw them.
///
/// The scan looks at each path without following links, but files are read later, some
/// of them minutes later, and a path can be something else by then. In a backup of
/// `/home` run as root, a user could swap a file of theirs (or a folder on the way to it)
/// for a link to `/etc/shadow` in between: its contents would be stored under their name,
/// for them to restore. A named pipe put there would make the backup wait forever.
///
/// So a file is opened below the source it was found in, without following any link on
/// the way or at the end (the kernel refuses the whole path if there is one, with
/// `openat2`; on kernels without it, at least the file itself is never a followed link),
/// without blocking, and is used only if it is a regular file. Links to and above the
/// source itself are followed, as the scan follows them: those are the user's own choice
/// of what to back up. With a profile that follows links, files are opened as before
/// (still never a pipe or device).
#[derive(Debug, Clone)]
pub struct SourceOpener {
    roots: Vec<std::path::PathBuf>,
    follow: bool,
}

impl SourceOpener {
    /// For a backup of `sources`; `follow_links`: the profile copies what links point to.
    pub fn new(sources: &[std::path::PathBuf], follow_links: bool) -> Self {
        let mut roots: Vec<_> = sources.to_vec();
        // The innermost source first, when one lies inside another.
        roots.sort_by_key(|r| std::cmp::Reverse(r.as_os_str().len()));
        Self {
            roots,
            follow: follow_links,
        }
    }

    pub fn open(&self, path: &Path) -> io::Result<File> {
        use rustix::fs::{Mode, OFlags, ResolveFlags};
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
        let fd = if self.follow {
            rustix::fs::open(path, flags, Mode::empty())?
        } else {
            let below = self
                .roots
                .iter()
                .find_map(|r| Some((r, path.strip_prefix(r).ok()?)))
                .filter(|(_, rel)| !rel.as_os_str().is_empty());
            let strict = below.and_then(|(root, rel)| {
                let dir = rustix::fs::open(
                    root,
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .ok()?;
                match rustix::fs::openat2(
                    &dir,
                    rel,
                    flags | OFlags::NOFOLLOW,
                    Mode::empty(),
                    ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::BENEATH,
                ) {
                    // A kernel (or sandbox) without openat2: the plain way, below.
                    Err(Errno::NOSYS | Errno::INVAL | Errno::PERM) => None,
                    r => Some(r),
                }
            });
            match strict {
                Some(r) => r,
                // The source itself (a single file given as a source may be a link), or
                // no openat2: the file itself is still never a followed link.
                None if below.is_none() && self.roots.iter().any(|r| r == path) => {
                    rustix::fs::open(path, flags, Mode::empty())
                }
                None => rustix::fs::open(path, flags | OFlags::NOFOLLOW, Mode::empty()),
            }
            .map_err(|e| match e {
                Errno::LOOP | Errno::XDEV => io::Error::other(
                    "became a link (or lies behind one) since it was looked at; not backed up",
                ),
                e => e.into(),
            })?
        };
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::other(
                "is no longer a regular file (it changed since it was looked at); not backed up",
            ));
        }
        Ok(file)
    }
}

/// Open regular file `rel` below folder `root` for reading, following no link on the
/// way from `root` (`root` itself may be reached through links: it is the caller's own
/// choice). Never a pipe or a device, and never waited on.
pub fn open_below(root: &Path, rel: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let dir = rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    open_below_dir(dir, rel)
}

/// [`open_below`] for a folder that is already open: whatever is done to its path
/// meanwhile (renamed away, another put in its place), this reads from the folder that
/// was opened.
pub fn open_below_dir(dir: std::os::fd::OwnedFd, rel: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
    let fd = match rustix::fs::openat2(
        &dir,
        rel,
        flags | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::BENEATH,
    ) {
        // A kernel (or sandbox) without openat2: one folder at a time, each opened
        // without following.
        Err(Errno::NOSYS | Errno::INVAL | Errno::PERM) => {
            let mut at = dir;
            let mut parts = rel.components().peekable();
            loop {
                let Some(std::path::Component::Normal(c)) = parts.next() else {
                    return Err(io::Error::other("not a plain path below the snapshot"));
                };
                if parts.peek().is_none() {
                    break rustix::fs::openat(&at, c, flags | OFlags::NOFOLLOW, Mode::empty());
                }
                at = rustix::fs::openat(
                    &at,
                    c,
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(below_err)?;
            }
        }
        r => r,
    }
    .map_err(below_err)?;
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "is not a regular file in the backup's stored files; not restored",
        ));
    }
    Ok(file)
}

fn below_err(e: Errno) -> io::Error {
    match e {
        Errno::LOOP | Errno::XDEV | Errno::NOTDIR => io::Error::other(
            "is a link, or lies behind one, in the backup's stored files (damaged or forged); not restored",
        ),
        e => e.into(),
    }
}

/// Create a special file (FIFO or device node).
pub fn make_special(dst: &Path, e: &Entry) -> io::Result<()> {
    use rustix::fs::{FileType, Mode};
    let ft = match e.kind {
        Kind::Fifo => FileType::Fifo,
        Kind::CharDevice => FileType::CharacterDevice,
        Kind::BlockDevice => FileType::BlockDevice,
        _ => return Ok(()),
    };
    rustix::fs::mknodat(CWD, dst, ft, Mode::from_raw_mode(0o600), e.rdev).map_err(Into::into)
}

/// Remove `path` whatever it is, including directory trees containing read-only
/// directories (which `remove_dir_all` can't descend into for deletion).
pub fn remove_force(path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            make_tree_writable(path)?;
            fs::remove_dir_all(path)
        }
    }
}

/// Give the owner rwx on every directory below (and including) `dir`.
pub fn make_tree_writable(dir: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Ok(());
    }
    if meta.mode() & 0o700 != 0o700 {
        fs::set_permissions(
            dir,
            fs::Permissions::from_mode((meta.mode() & 0o7777) | 0o700),
        )?;
    }
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        if ent.file_type()?.is_dir() {
            make_tree_writable(&ent.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backup reads the file the scan saw: not what a link put in its place (or in
    /// place of a folder on the way) points to, and never a pipe it would wait on.
    #[test]
    fn sources_are_opened_without_following_links() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let secret = t.path().join("secret");
        fs::create_dir(&secret).unwrap();
        fs::write(secret.join("shadow"), "root's").unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("dir/file"), "mine").unwrap();
        let opener = SourceOpener::new(std::slice::from_ref(&src), false);
        let read = |o: &SourceOpener, p: &Path| {
            o.open(p).and_then(|mut f| {
                let mut s = String::new();
                f.read_to_string(&mut s).map(|_| s)
            })
        };
        assert_eq!(read(&opener, &src.join("dir/file")).unwrap(), "mine");

        // The file becomes a link to someone else's.
        symlink(secret.join("shadow"), src.join("dir/swapped")).unwrap();
        assert!(read(&opener, &src.join("dir/swapped")).is_err());
        // A folder on the way becomes a link.
        symlink(&secret, src.join("way")).unwrap();
        assert!(read(&opener, &src.join("way/shadow")).is_err());
        // A named pipe: refused at once, not waited on.
        rustix::fs::mknodat(
            CWD,
            src.join("pipe"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(read(&opener, &src.join("pipe")).is_err());

        // A profile that follows links reads through them, as it always did.
        let following = SourceOpener::new(std::slice::from_ref(&src), true);
        assert_eq!(
            read(&following, &src.join("dir/swapped")).unwrap(),
            "root's"
        );
        assert!(read(&following, &src.join("pipe")).is_err());

        // The source itself may be reached through a link: that is the user's choice.
        symlink(&src, t.path().join("alias")).unwrap();
        let alias = t.path().join("alias");
        let via = SourceOpener::new(std::slice::from_ref(&alias), false);
        assert_eq!(read(&via, &alias.join("dir/file")).unwrap(), "mine");
        assert!(read(&via, &alias.join("dir/swapped")).is_err());
        // A single file as the source, itself a link.
        let one = t.path().join("one");
        symlink(src.join("dir/file"), &one).unwrap();
        let single = SourceOpener::new(std::slice::from_ref(&one), false);
        assert_eq!(read(&single, &one).unwrap(), "mine");
    }

    #[test]
    fn copy_and_hash() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let p = Progress::new();
        let c = Copier::default();
        let n = c.copy(&src, &t.path().join("dst"), &p).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(fs::read(t.path().join("dst")).unwrap(), data);
        assert_eq!(hash_file(&src).unwrap(), *blake3::hash(&data).as_bytes());
        // Refuses to overwrite.
        assert!(c.copy(&src, &t.path().join("dst"), &p).is_err());
    }

    #[test]
    fn removes_read_only_trees() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("a/b");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f"), "x").unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(t.path().join("a"), fs::Permissions::from_mode(0o500)).unwrap();
        remove_force(&t.path().join("a")).unwrap();
        assert!(!t.path().join("a").exists());
    }

    #[test]
    fn mtime_on_symlink() {
        let t = tempfile::tempdir().unwrap();
        let l = t.path().join("l");
        std::os::unix::fs::symlink("nowhere", &l).unwrap();
        set_mtime(&l, 1_000_000_000, 5).unwrap();
        let m = fs::symlink_metadata(&l).unwrap();
        assert_eq!((m.mtime(), m.mtime_nsec()), (1_000_000_000, 5));
    }

    #[test]
    fn sparse_files_stay_sparse() {
        use std::os::unix::fs::{FileExt, MetadataExt};
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("disk.img");
        let f = File::create(&src).unwrap();
        f.set_len(64 << 20).unwrap();
        f.write_all_at(b"start", 0).unwrap();
        f.write_all_at(&[7u8; 8192], 32 << 20).unwrap();
        drop(f);
        if !is_sparse(&fs::metadata(&src).unwrap()) {
            eprintln!("this filesystem doesn't make sparse files; skipping");
            return;
        }
        for copy_range in [true, false] {
            let c = Copier {
                reflink: AtomicBool::new(false),
                copy_range: AtomicBool::new(copy_range),
            };
            let dst = t.path().join(format!("copy-{copy_range}"));
            let p = Progress::new();
            c.copy(&src, &dst, &p).unwrap();
            let m = fs::metadata(&dst).unwrap();
            assert_eq!(m.len(), 64 << 20);
            assert!(
                m.blocks() * 512 < 4 << 20,
                "copy is sparse: {} blocks",
                m.blocks()
            );
            assert_eq!(fs::read(&dst).unwrap(), fs::read(&src).unwrap());
            assert_eq!(
                p.bytes_done.load(Relaxed),
                64 << 20,
                "progress counts the whole file"
            );
        }
    }

    #[test]
    fn data_segments_of_sparse_and_dense_files() {
        use std::os::unix::fs::FileExt;
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("disk.img");
        let f = File::create(&src).unwrap();
        f.set_len(64 << 20).unwrap();
        f.write_all_at(&[1u8; 4096], 0).unwrap();
        f.write_all_at(&[7u8; 8192], 32 << 20).unwrap();
        drop(f);
        let f = File::open(&src).unwrap();
        if !is_sparse(&f.metadata().unwrap()) {
            eprintln!("this filesystem doesn't make sparse files; skipping");
            return;
        }
        let runs = data_segments(&f, 64 << 20).unwrap().unwrap();
        // Filesystems round to their block size; the data written is inside the runs.
        let covered = |off: u64, n: u64| runs.iter().any(|&(s, l)| s <= off && off + n <= s + l);
        assert!(covered(0, 4096) && covered(32 << 20, 8192), "{runs:?}");
        let data: u64 = runs.iter().map(|r| r.1).sum();
        assert!(data < 4 << 20, "mostly holes: {runs:?}");

        let dense = t.path().join("dense");
        fs::write(&dense, vec![3u8; 100_000]).unwrap();
        assert_eq!(
            data_segments(&File::open(&dense).unwrap(), 100_000).unwrap(),
            None
        );
    }

    /// A snapshot's stored file is opened below its folder without following links: one
    /// planted there (in the file's place, or a folder on the way) is refused.
    #[test]
    fn open_below_follows_no_link() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        fs::create_dir_all(data.join("home/me")).unwrap();
        fs::write(data.join("home/me/a.txt"), "stored").unwrap();
        let secret = t.path().join("secret");
        fs::create_dir(&secret).unwrap();
        fs::write(secret.join("shadow"), "root only").unwrap();
        std::os::unix::fs::symlink(secret.join("shadow"), data.join("home/me/link.txt")).unwrap();
        std::os::unix::fs::symlink(&secret, data.join("home/dir")).unwrap();

        let mut text = String::new();
        open_below(&data, Path::new("home/me/a.txt"))
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "stored");
        for planted in ["home/me/link.txt", "home/dir/shadow", "../secret/shadow"] {
            assert!(open_below(&data, Path::new(planted)).is_err(), "{planted}");
        }
        // The folder itself may be reached through a link (`latest`).
        let latest = t.path().join("latest");
        std::os::unix::fs::symlink(&data, &latest).unwrap();
        assert!(open_below(&latest, Path::new("home/me/a.txt")).is_ok());
    }
}
