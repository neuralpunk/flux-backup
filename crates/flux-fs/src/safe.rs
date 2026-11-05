//! Symlink-safe access to restore destinations.
//!
//! A restore running as root writes into directories that other users may control (their
//! home folders, or a folder chosen as the destination). If it resolved paths the ordinary
//! way, a user could swap a directory for a symlink and redirect root's writes, for
//! example into `/etc`. So destinations are opened here one component at a time, relative
//! to directory file descriptors, never following a symlink unless its owner is trusted;
//! the restore then works with `*at()` calls on those descriptors and with descriptors of
//! the files it creates itself.
//!
//! Policy:
//!
//! * not root: follow symlinks as usual (there's nobody to escalate to)
//! * root: follow a symlink only if it's owned by root, or by the user who asked for the
//!   restore (`PKEXEC_UID`/`SUDO_UID`), whose own links are their intention; anything
//!   else is refused with an error that names the link
//!
//! Directory descriptors are `O_PATH`: enough for `*at()` calls, not for reading.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;

/// Symlinks followed at most while resolving one path (the kernel's own limit).
const MAX_LINKS: usize = 40;

/// Which symlinks may be followed in a destination path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkPolicy {
    /// Ordinary path resolution.
    FollowAll,
    /// Follow only symlinks owned by one of these users.
    TrustOwners(Vec<u32>),
}

impl LinkPolicy {
    /// The policy for this process: [`LinkPolicy::FollowAll`] unless running as root.
    pub fn for_process() -> Self {
        if !flux_core::sys::is_root() {
            return Self::FollowAll;
        }
        let mut owners = vec![0];
        if let Some(uid) = invoking_uid()
            && uid != 0
        {
            owners.push(uid);
        }
        Self::TrustOwners(owners)
    }

    fn trusts(&self, uid: u32) -> bool {
        match self {
            Self::FollowAll => true,
            Self::TrustOwners(owners) => owners.contains(&uid),
        }
    }
}

/// The unprivileged user behind `pkexec` or `sudo`, if any.
pub fn invoking_uid() -> Option<u32> {
    ["PKEXEC_UID", "SUDO_UID"]
        .iter()
        .find_map(|v| std::env::var(v).ok()?.parse().ok())
}

/// A directory opened for `*at()` calls (an `O_PATH` descriptor).
pub type DirFd = Arc<OwnedFd>;

/// Opens destination directories according to a [`LinkPolicy`].
#[derive(Debug)]
pub struct DestTree {
    policy: LinkPolicy,
    /// Tells trees apart in the per-thread cache.
    serial: u64,
}

static SERIAL: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The last directory this thread resolved (tree serial, path): restores write many
    /// siblings in a row.
    static LAST: RefCell<Option<(u64, PathBuf, DirFd)>> = const { RefCell::new(None) };
}

fn refused(path: &Path, link: &Path, uid: u32) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{}: not following the symlink {} (owned by uid {uid}); as root, Flux only follows symlinks owned by root or by you. Restore into another folder instead.",
            path.display(),
            link.display()
        ),
    )
}

impl DestTree {
    pub fn new(policy: LinkPolicy) -> Self {
        Self {
            policy,
            serial: SERIAL.fetch_add(1, Relaxed),
        }
    }

    pub fn policy(&self) -> &LinkPolicy {
        &self.policy
    }

    /// Open directory `path` (absolute), creating missing directories (mode 0700) when
    /// `create`.
    pub fn dir(&self, path: &Path, create: bool) -> io::Result<DirFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: not an absolute path", path.display()),
            ));
        }
        if let Some(fd) = LAST.with_borrow(|l| {
            l.as_ref()
                .filter(|(s, p, _)| *s == self.serial && p == path)
                .map(|(_, _, fd)| fd.clone())
        }) {
            return Ok(fd);
        }
        let fd = Arc::new(self.resolve(path, create)?);
        LAST.with_borrow_mut(|l| *l = Some((self.serial, path.to_owned(), fd.clone())));
        Ok(fd)
    }

    /// The directory containing `path` and the final component's name.
    pub fn parent(&self, path: &Path, create: bool) -> io::Result<(DirFd, OsString)> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: has no parent directory", path.display()),
            ));
        };
        Ok((self.dir(parent, create)?, name.to_owned()))
    }

    /// Forget this thread's cached directory (after replacing a directory).
    pub fn forget(&self) {
        LAST.with_borrow_mut(|l| *l = None);
    }

    fn resolve(&self, path: &Path, create: bool) -> io::Result<OwnedFd> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fast = match self.policy {
            LinkPolicy::FollowAll => rustix::fs::open(path, flags, Mode::empty()),
            // The kernel refuses every symlink atomically; only paths that contain links
            // (or don't exist yet) need the slow, component-wise walk.
            LinkPolicy::TrustOwners(_) => rustix::fs::openat2(
                rustix::fs::CWD,
                path,
                flags,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            ),
        };
        match fast {
            Ok(fd) => return Ok(fd),
            Err(Errno::NOENT | Errno::LOOP | Errno::NOSYS | Errno::INVAL | Errno::XDEV) => {}
            Err(e) => return Err(with_path(e.into(), path)),
        }
        self.walk(path, create).map_err(|e| with_path(e, path))
    }

    /// Resolve `path` from `/` one component at a time, never letting the kernel follow a
    /// symlink: links are read and checked here, then their targets are walked the same way.
    fn walk(&self, path: &Path, create: bool) -> io::Result<OwnedFd> {
        let root = rustix::fs::open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        // The directories walked so far; `..` pops one, so it never escapes what we opened.
        let mut chain: Vec<OwnedFd> = vec![root];
        let mut shown = PathBuf::from("/");
        let mut todo: Vec<OsString> = components(path)?;
        todo.reverse();
        let mut links = 0;
        while let Some(c) = todo.pop() {
            if c == ".." {
                if chain.len() > 1 {
                    chain.pop();
                }
                shown.pop();
                continue;
            }
            let cur = chain.last().expect("root stays");
            match rustix::fs::openat(
                cur,
                c.as_os_str(),
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => {
                    let st = rustix::fs::fstat(&fd)?;
                    match FileType::from_raw_mode(st.st_mode) {
                        FileType::Directory => {
                            shown.push(&c);
                            chain.push(fd);
                        }
                        FileType::Symlink => {
                            let link = shown.join(&c);
                            if !self.policy.trusts(st.st_uid) {
                                return Err(refused(path, &link, st.st_uid));
                            }
                            links += 1;
                            if links > MAX_LINKS {
                                return Err(Errno::LOOP.into());
                            }
                            // Read the link through its own descriptor: the one we checked.
                            let target = rustix::fs::readlinkat(&fd, "", Vec::new())?;
                            let target = PathBuf::from(OsStr::from_bytes(target.as_bytes()));
                            if target.is_absolute() {
                                chain.truncate(1);
                                shown = PathBuf::from("/");
                            }
                            let mut more = components(&target)?;
                            more.reverse();
                            todo.extend(more);
                        }
                        _ => return Err(Errno::NOTDIR.into()),
                    }
                }
                Err(Errno::NOENT) if create => {
                    match rustix::fs::mkdirat(cur, c.as_os_str(), Mode::from_raw_mode(0o700)) {
                        Ok(()) | Err(Errno::EXIST) => {}
                        Err(e) => return Err(e.into()),
                    }
                    todo.push(c); // open what we just made (still without following links)
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(chain.pop().expect("root stays"))
    }
}

/// `path`'s components as names and `..` (`.` and the root are dropped).
fn components(path: &Path) -> io::Result<Vec<OsString>> {
    let mut out = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(n) => out.push(n.to_owned()),
            Component::ParentDir => out.push("..".into()),
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(out)
}

fn with_path(e: io::Error, path: &Path) -> io::Error {
    if e.kind() == io::ErrorKind::PermissionDenied && e.to_string().contains("symlink") {
        return e;
    }
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// A path that reaches `name` inside the directory `dir` through `/proc/self/fd`, for the
/// few APIs that only take paths. Intermediate resolution goes through the descriptor, so
/// it can't be redirected; whether the final component is followed is up to the caller.
pub fn proc_path(dir: &impl AsFd, name: &OsStr) -> PathBuf {
    let mut p = PathBuf::from(format!("/proc/self/fd/{}", dir.as_fd().as_raw_fd()));
    if !name.is_empty() {
        p.push(name);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn me() -> u32 {
        flux_core::sys::euid()
    }

    fn strict() -> DestTree {
        // Trust only some other user: every link the test creates is untrusted.
        DestTree::new(LinkPolicy::TrustOwners(vec![me().wrapping_add(4242)]))
    }

    #[test]
    fn creates_missing_directories() {
        let t = tempfile::tempdir().unwrap();
        let tree = strict();
        let p = t.path().join("a/b/c");
        tree.dir(&p, true).unwrap();
        assert!(p.is_dir());
        assert!(tree.dir(&t.path().join("x/y"), false).is_err());
    }

    #[test]
    fn refuses_untrusted_links_but_follows_trusted_ones() {
        let t = tempfile::tempdir().unwrap();
        // A symlink inside the temp dir: /tmp itself may be a link on some systems, so make
        // sure the resolution before it is clean by canonicalizing.
        let base = t.path().canonicalize().unwrap();
        std::fs::create_dir(base.join("real")).unwrap();
        symlink(base.join("real"), base.join("link")).unwrap();
        let err = strict().dir(&base.join("link/inside"), true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert!(err.to_string().contains("link"), "{err}");
        assert!(
            !base.join("real/inside").exists(),
            "nothing was created through it"
        );

        let trusting = DestTree::new(LinkPolicy::TrustOwners(vec![me()]));
        trusting.dir(&base.join("link/inside"), true).unwrap();
        assert!(base.join("real/inside").is_dir());

        // Relative links with `..` are walked through descriptors too.
        symlink("../real", base.join("real/up")).unwrap();
        trusting.forget();
        trusting.dir(&base.join("real/up/inside"), false).unwrap();
        DestTree::new(LinkPolicy::FollowAll)
            .dir(&base.join("link/inside"), false)
            .unwrap();
    }

    #[test]
    fn proc_paths() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("f"), "x").unwrap();
        let d = DestTree::new(LinkPolicy::FollowAll)
            .dir(t.path(), false)
            .unwrap();
        assert_eq!(
            std::fs::read(proc_path(&*d, OsStr::new("f"))).unwrap(),
            b"x"
        );
    }
}
