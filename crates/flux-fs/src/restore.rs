//! Restore paths from a snapshot, to their original location or into another directory.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;
use serde::Serialize;

use flux_core::progress::{Event, Phase, Progress, Reporter};

use crate::entry::{Entry, Kind, is_clean_path, is_within};
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use crate::fsops::{
    ApplyPolicy, Copier, XattrPolicy, apply_link_metadata_at, apply_metadata_fd,
    apply_special_metadata_at, make_special_at, remove_force,
};
use crate::safe::{DestTree, LinkPolicy};
use crate::snapshot::{FileError, SnapshotMeta};

/// Where a snapshot's entries and file contents come from. Each engine implements this.
pub trait RestoreSource: Sync {
    /// The snapshot's entries, sorted by path.
    fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<Entry>> + '_>>;
    /// Write the contents of regular file `e` into `out`, a new empty file. Returns the
    /// bytes written; updates `progress.bytes_done` as it goes.
    fn write_file(
        &self,
        e: &Entry,
        out: &File,
        copier: &Copier,
        progress: &Progress,
    ) -> io::Result<u64>;
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("{0} is not in snapshot {1}")]
    NotInSnapshot(PathBuf, String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cancelled")]
    Cancelled,
}

pub struct RestoreRequest<'a> {
    pub snapshot: &'a SnapshotMeta,
    /// Original absolute paths to restore (files or directories).
    pub paths: Vec<PathBuf>,
    /// Restore into this directory instead of the original location. Each requested path
    /// lands at `to/<its name>`.
    pub to: Option<PathBuf>,
    /// Move existing differing files aside (`name.flux-<snapshot>`) instead of overwriting.
    pub keep_existing: bool,
    /// Only report what would happen.
    pub dry_run: bool,
    pub xattrs: XattrPolicy,
    /// Which symlinks in destination paths may be followed; usually
    /// [`LinkPolicy::for_process`].
    pub links: LinkPolicy,
}

#[derive(Debug, Default, Serialize)]
pub struct RestoreOutcome {
    pub restored: u64,
    pub bytes: u64,
    pub skipped_identical: u64,
    pub moved_aside: Vec<PathBuf>,
    /// Files whose setuid/setgid bits were dropped because root restored them into a
    /// folder owned by someone else.
    pub setid_dropped: u64,
    pub errors: Vec<FileError>,
    /// For dry runs: destination paths that would be written.
    pub would_write: Vec<PathBuf>,
}

pub fn restore(
    source: &dyn RestoreSource,
    req: &RestoreRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<RestoreOutcome, RestoreError> {
    progress.set_phase(Phase::Scanning);
    let io_err = |source| RestoreError::Io {
        path: PathBuf::from(&req.snapshot.id),
        source,
    };

    let (items, unsafe_paths) = select(source.entries().map_err(io_err)?, req)?;

    let total_bytes: u64 = items
        .iter()
        .filter(|(e, _)| e.kind == Kind::File)
        .map(|(e, _)| e.size)
        .sum();
    progress.entries_total.store(items.len() as u64, Relaxed);
    progress.bytes_total.store(total_bytes, Relaxed);

    let mut out = RestoreOutcome::default();
    for fe in &unsafe_paths {
        report(Event::Warning {
            path: fe.path.clone(),
            message: fe.message.clone(),
        });
        progress.warnings.fetch_add(1, Relaxed);
    }
    if req.dry_run {
        out.errors = unsafe_paths;
        out.would_write = items.into_iter().map(|(_, d)| d).collect();
        return Ok(out);
    }

    progress.set_phase(Phase::Transferring);
    let policy = ApplyPolicy::new(req.xattrs);
    let tree = DestTree::new(req.links.clone());
    let copier = Copier::default();
    let suffix = format!("flux-{}", req.snapshot.id);
    let errors = Mutex::new(Vec::new());
    let moved = Mutex::new(Vec::new());
    let restored = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let setid_dropped = AtomicU64::new(0);
    let warn = |path: &Path, err: &dyn std::fmt::Display| {
        let fe = FileError {
            path: path.display().to_string(),
            message: err.to_string(),
        };
        report(Event::Warning {
            path: fe.path.clone(),
            message: fe.message.clone(),
        });
        progress.warnings.fetch_add(1, Relaxed);
        errors.lock().unwrap().push(fe);
    };

    // When root restores into a folder that someone else owns, that person must not end up
    // with setuid programs or device nodes they could use: drop those bits and skip devices.
    let mut untrusted_dest = false;
    if let Some(to) = &req.to {
        let dir = tree.dir(to, true).map_err(|source| RestoreError::Io {
            path: to.clone(),
            source,
        })?;
        if policy.privileged {
            let st = rustix::fs::fstat(&*dir).map_err(|e| RestoreError::Io {
                path: to.clone(),
                source: e.into(),
            })?;
            untrusted_dest = st.st_uid != 0;
        }
    }
    let mode_mask = if untrusted_dest { 0o6000 } else { 0 };
    let ctx = Ctx {
        tree: &tree,
        suffix: &suffix,
        keep: req.keep_existing,
        privileged: policy.privileged,
        moved: &moved,
    };

    // Directories first (path order puts parents first).
    for (_, dest) in items.iter().filter(|(e, _)| e.kind == Kind::Dir) {
        if let Err(err) = ctx.make_dir(dest) {
            warn(dest, &err);
        }
        restored.fetch_add(1, Relaxed);
        progress.entries_done.fetch_add(1, Relaxed);
    }

    // Hard-link groups: the first member is copied, later members are linked to it.
    let mut leaders: HashMap<(u64, u64), usize> = HashMap::new();
    let mut follower_of = vec![None; items.len()];
    for (i, (e, _)) in items.iter().enumerate() {
        if e.kind == Kind::File && e.nlink > 1 {
            let l = *leaders.entry((e.dev, e.ino)).or_insert(i);
            if l != i {
                follower_of[i] = Some(l);
            }
        }
    }

    let restore_one = |i: usize| -> io::Result<bool> {
        let (e, dest) = &items[i];
        let (dir, name) = tree.parent(dest, true)?;
        let dir = dir.as_fd();
        match rustix::fs::statat(dir, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => {
                if e.kind == Kind::File
                    && FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
                    && st.st_size as u64 == e.size
                    && st.st_mtime == e.mtime
                    && st.st_mtime_nsec as u32 == e.mtime_nsec
                {
                    progress.bytes_done.fetch_add(e.size, Relaxed);
                    return Ok(false); // identical: leave it
                }
                ctx.set_aside(dir, &name, dest, FileType::from_raw_mode(st.st_mode))?;
            }
            Err(Errno::NOENT) => {}
            Err(err) => return Err(err.into()),
        }
        match e.kind {
            Kind::File => {
                if let Some(l) = follower_of[i] {
                    let (ldir, lname) = tree.parent(&items[l].1, false)?;
                    rustix::fs::linkat(&*ldir, &lname, dir, &name, AtFlags::empty())?;
                    return Ok(true);
                }
                // O_EXCL | O_NOFOLLOW: a new file of our own, never something planted there.
                let fd = rustix::fs::openat(
                    dir,
                    &name,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o600),
                )?;
                let file = File::from(fd);
                let n = source.write_file(e, &file, &copier, progress)?;
                bytes.fetch_add(n, Relaxed);
                if e.mode & mode_mask != 0 {
                    setid_dropped.fetch_add(1, Relaxed);
                }
                apply_metadata_fd(file.as_fd(), e, &policy, mode_mask)?;
            }
            Kind::Symlink => {
                let t = std::ffi::OsStr::from_bytes(e.link.as_deref().unwrap_or_default());
                rustix::fs::symlinkat(t, dir, &name)?;
                apply_link_metadata_at(dir, &name, e, &policy)?;
            }
            Kind::Fifo => {
                make_special_at(dir, &name, e)?;
                apply_special_metadata_at(dir, &name, e, &policy, mode_mask)?;
            }
            Kind::CharDevice | Kind::BlockDevice if policy.privileged && !untrusted_dest => {
                make_special_at(dir, &name, e)?;
                apply_special_metadata_at(dir, &name, e, &policy, mode_mask)?;
            }
            Kind::CharDevice | Kind::BlockDevice if untrusted_dest => {
                return Err(io::Error::other(
                    "device node not restored into a folder owned by another user",
                ));
            }
            _ => return Ok(false),
        }
        Ok(true)
    };

    let run = |i: usize| {
        if progress.is_cancelled() {
            return;
        }
        match restore_one(i) {
            Ok(true) => {
                restored.fetch_add(1, Relaxed);
            }
            Ok(false) => {
                skipped.fetch_add(1, Relaxed);
            }
            Err(err) => warn(&items[i].1, &err),
        }
        progress.entries_done.fetch_add(1, Relaxed);
    };
    let non_dirs: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].0.kind != Kind::Dir)
        .collect();
    non_dirs
        .par_iter()
        .filter(|&&i| follower_of[i].is_none())
        .for_each(|&i| run(i));
    non_dirs
        .par_iter()
        .filter(|&&i| follower_of[i].is_some())
        .for_each(|&i| run(i));

    if progress.is_cancelled() {
        return Err(RestoreError::Cancelled);
    }

    // Directory metadata last, deepest first, through descriptors of the directories.
    for (e, dest) in items.iter().rev().filter(|(e, _)| e.kind == Kind::Dir) {
        let r = tree.parent(dest, false).and_then(|(dir, name)| {
            let fd = rustix::fs::openat(
                &*dir,
                &name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            if e.mode & mode_mask != 0 {
                setid_dropped.fetch_add(1, Relaxed);
            }
            apply_metadata_fd(fd.as_fd(), e, &policy, mode_mask)
        });
        if let Err(err) = r {
            warn(dest, &err);
        }
    }

    progress.set_phase(Phase::Done);
    out.restored = restored.into_inner();
    out.skipped_identical = skipped.into_inner();
    out.bytes = bytes.into_inner();
    out.setid_dropped = setid_dropped.into_inner();
    out.errors = unsafe_paths;
    out.errors.extend(errors.into_inner().unwrap());
    out.moved_aside = moved.into_inner().unwrap();
    Ok(out)
}

/// The entries a request covers, with their destinations, in path order; and the paths a
/// sound snapshot never contains (`..`, absolute…), which are never read or written.
type Selected = (Vec<(Entry, PathBuf)>, Vec<FileError>);

fn select(
    entries: impl Iterator<Item = io::Result<Entry>>,
    req: &RestoreRequest<'_>,
) -> Result<Selected, RestoreError> {
    let io_err = |source| RestoreError::Io {
        path: PathBuf::from(&req.snapshot.id),
        source,
    };
    let wanted: Vec<Vec<u8>> = req
        .paths
        .iter()
        .map(|p| {
            let b = p.as_os_str().as_bytes();
            let b = b.strip_prefix(b"/").unwrap_or(b);
            b.strip_suffix(b"/").unwrap_or(b).to_vec()
        })
        .collect();

    // (entry, destination) pairs in path order.
    let mut items: Vec<(Entry, PathBuf)> = Vec::new();
    let mut found = vec![false; wanted.len()];
    // Paths a sound snapshot never contains (`..`, absolute…): never read or written.
    let mut unsafe_paths: Vec<FileError> = Vec::new();
    for e in entries {
        let e = e.map_err(io_err)?;
        if !is_clean_path(&e.path) {
            unsafe_paths.push(FileError {
                path: String::from_utf8_lossy(&e.path).into_owned(),
                message: "unsafe path in the snapshot (damaged or forged); skipped".into(),
            });
            continue;
        }
        for (i, w) in wanted.iter().enumerate() {
            if is_within(&e.path, w) {
                found[i] = true;
                let dest = destination(&e, w, req.to.as_deref());
                items.push((e, dest));
                break;
            }
        }
    }
    if let Some(i) = found.iter().position(|f| !f) {
        return Err(RestoreError::NotInSnapshot(
            req.paths[i].clone(),
            req.snapshot.id.clone(),
        ));
    }
    Ok((items, unsafe_paths))
}

/// At most this many paths of each kind in a [`RestorePreview`].
pub const PREVIEW_PATHS: usize = 100;

/// What a restore would do, found without changing anything.
#[derive(Debug, Default, Serialize)]
pub struct RestorePreview {
    /// Items that don't exist at their destination yet (folders not counted).
    pub new: u64,
    /// Items that exist and differ: replaced, or renamed to `name.flux-<snapshot>` first
    /// when `keep_existing` is set.
    pub differ: u64,
    /// Files already identical (same size and modification time): left alone.
    pub identical: u64,
    /// Folders that would be created.
    pub new_folders: u64,
    /// Bytes of the files that would be written.
    pub bytes: u64,
    pub keep_existing: bool,
    /// The first [`PREVIEW_PATHS`] differing and new destinations.
    pub differ_paths: Vec<PathBuf>,
    pub new_paths: Vec<PathBuf>,
    /// Damaged or forged paths in the snapshot, which a restore skips.
    pub unsafe_paths: u64,
}

/// What [`restore`] would do with `req`: compares the snapshot's entries with what is at
/// each destination now, by the same rule restore uses (a regular file with the same size
/// and modification time is identical). Reads nothing but metadata.
pub fn preview(
    entries: impl Iterator<Item = io::Result<Entry>>,
    req: &RestoreRequest<'_>,
) -> Result<RestorePreview, RestoreError> {
    use std::os::unix::fs::MetadataExt;
    let (items, unsafe_paths) = select(entries, req)?;
    let mut out = RestorePreview {
        keep_existing: req.keep_existing,
        unsafe_paths: unsafe_paths.len() as u64,
        ..Default::default()
    };
    for (e, dest) in items {
        let size = if e.kind == Kind::File { e.size } else { 0 };
        match fs::symlink_metadata(&dest) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if e.kind == Kind::Dir {
                    out.new_folders += 1;
                } else {
                    out.new += 1;
                    out.bytes += size;
                    if out.new_paths.len() < PREVIEW_PATHS {
                        out.new_paths.push(dest);
                    }
                }
            }
            Ok(m) if e.kind == Kind::Dir && m.is_dir() => {}
            Ok(m)
                if e.kind == Kind::File
                    && m.file_type().is_file()
                    && m.len() == e.size
                    && m.mtime() == e.mtime
                    && m.mtime_nsec() as u32 == e.mtime_nsec =>
            {
                out.identical += 1;
            }
            // Different, or there but unreadable: either way restore has to deal with it.
            _ => {
                out.differ += 1;
                out.bytes += size;
                if out.differ_paths.len() < PREVIEW_PATHS {
                    out.differ_paths.push(dest);
                }
            }
        }
    }
    Ok(out)
}

/// What every destination operation needs.
struct Ctx<'a> {
    tree: &'a DestTree,
    suffix: &'a str,
    keep: bool,
    privileged: bool,
    moved: &'a Mutex<Vec<PathBuf>>,
}

impl Ctx<'_> {
    /// Make sure `dest` is a directory, setting aside whatever else is there.
    fn make_dir(&self, dest: &Path) -> io::Result<()> {
        let (dir, name) = self.tree.parent(dest, true)?;
        match rustix::fs::statat(&*dir, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Directory => return Ok(()),
            Ok(st) => self.set_aside(
                dir.as_fd(),
                &name,
                dest,
                FileType::from_raw_mode(st.st_mode),
            )?,
            Err(Errno::NOENT) => {}
            Err(e) => return Err(e.into()),
        }
        match rustix::fs::mkdirat(&*dir, &name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(Errno::EXIST) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Make room for `name` in `dir`: rename it to `name.<suffix>` (unique) or delete it.
    fn set_aside(
        &self,
        dir: BorrowedFd<'_>,
        name: &OsStr,
        dest: &Path,
        kind: FileType,
    ) -> io::Result<()> {
        if kind == FileType::Directory {
            self.tree.forget();
        }
        if !self.keep {
            if kind != FileType::Directory {
                return rustix::fs::unlinkat(dir, name, AtFlags::empty()).map_err(Into::into);
            }
            let path = crate::safe::proc_path(&dir, name);
            // Root needs no permission fixes, and std's remove_dir_all never follows links;
            // users may need read-only directories made writable first.
            return if self.privileged {
                fs::remove_dir_all(path)
            } else {
                remove_force(&path)
            };
        }
        let mut aside = name.to_owned();
        aside.push(format!(".{}", self.suffix));
        let base = aside.clone();
        let mut n = 2;
        while rustix::fs::statat(dir, &aside, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
            aside = base.clone();
            aside.push(format!("-{n}"));
            n += 1;
        }
        rustix::fs::renameat(dir, name, dir, &aside)?;
        self.moved.lock().unwrap().push(dest.with_file_name(&aside));
        Ok(())
    }
}

/// Where entry `e` (selected via the requested path `wanted`) is restored to.
fn destination(e: &Entry, wanted: &[u8], to: Option<&Path>) -> PathBuf {
    let Some(to) = to else { return e.abs_path() };
    // Keep the requested item's own name: restoring /home/me/Docs to /tmp/r gives /tmp/r/Docs.
    let base_len = wanted.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let rel = &e.path[base_len.min(e.path.len())..];
    to.join(std::ffi::OsStr::from_bytes(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::test_entry as entry;

    fn test_snapshot() -> SnapshotMeta {
        SnapshotMeta {
            format: 1,
            id: "s".into(),
            profile: "p".into(),
            host: "h".into(),
            user: "u".into(),
            started: jiff::Timestamp::UNIX_EPOCH,
            finished: jiff::Timestamp::UNIX_EPOCH,
            status: crate::snapshot::SnapshotStatus::Complete,
            name: None,
            pinned: false,
            sources: Vec::new(),
            stats: Default::default(),
            errors: Vec::new(),
            errors_total: 0,
            flux_version: "0".into(),
        }
    }

    #[test]
    fn clean_paths() {
        for ok in ["home", "home/me/a.txt", "a/.hidden", "a/..b", "a/b.."] {
            assert!(is_clean_path(ok.as_bytes()), "{ok}");
        }
        for bad in ["", "/etc", "a/../b", "..", "a/./b", "a//b", "a/", "a/\0b"] {
            assert!(!is_clean_path(bad.as_bytes()), "{bad:?}");
        }
    }

    /// A snapshot source with forged entries: a `..` path must be neither read nor written.
    #[test]
    fn forged_parent_paths_are_skipped() {
        struct Forged(Vec<Entry>);
        impl RestoreSource for Forged {
            fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<Entry>> + '_>> {
                Ok(Box::new(self.0.iter().cloned().map(Ok)))
            }
            fn write_file(
                &self,
                e: &Entry,
                out: &File,
                _copier: &Copier,
                _progress: &Progress,
            ) -> io::Result<u64> {
                assert!(is_clean_path(&e.path), "asked to read {:?}", e.path);
                let mut out = out;
                std::io::Write::write_all(&mut out, b"data")?;
                Ok(4)
            }
        }
        let t = tempfile::tempdir().unwrap();
        let mut good = entry("src/ok.txt", Kind::File);
        good.size = 4;
        good.mode = 0o644;
        let mut evil = entry("src/../../escaped.txt", Kind::File);
        evil.size = 4;
        evil.mode = 0o644;
        let mut dir = entry("src", Kind::Dir);
        dir.mode = 0o755;
        let source = Forged(vec![dir, evil, good]);
        let snap = test_snapshot();
        let to = t.path().join("a/b/dest");
        let out = restore(
            &source,
            &RestoreRequest {
                snapshot: &snap,
                paths: vec![PathBuf::from("/src")],
                to: Some(to.clone()),
                keep_existing: false,
                dry_run: false,
                xattrs: XattrPolicy {
                    acl: false,
                    other: false,
                },
                links: LinkPolicy::FollowAll,
            },
            &Progress::new(),
            &flux_core::progress::silent,
        )
        .unwrap();
        assert_eq!(std::fs::read(to.join("src/ok.txt")).unwrap(), b"data");
        assert!(!t.path().join("a/escaped.txt").exists());
        assert!(!t.path().join("a/b/escaped.txt").exists());
        assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
        assert!(out.errors[0].message.contains("unsafe path"));
    }

    #[test]
    fn destinations() {
        let e = entry("home/me/Docs/a/b.txt", Kind::File);
        assert_eq!(
            destination(&e, b"home/me/Docs", None),
            PathBuf::from("/home/me/Docs/a/b.txt")
        );
        assert_eq!(
            destination(&e, b"home/me/Docs", Some(Path::new("/tmp/r"))),
            PathBuf::from("/tmp/r/Docs/a/b.txt")
        );
        let top = entry("etc", Kind::Dir);
        assert_eq!(
            destination(&top, b"etc", Some(Path::new("/r"))),
            PathBuf::from("/r/etc")
        );
    }
    #[test]
    fn preview_sorts_new_differing_and_identical() {
        let t = tempfile::tempdir().unwrap();
        let root = t
            .path()
            .strip_prefix("/")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let at = |rel: &str| t.path().join(rel);
        std::fs::create_dir(at("d")).unwrap();
        std::fs::write(at("d/same.txt"), b"1234").unwrap();
        std::fs::write(at("d/changed.txt"), b"old").unwrap();
        let st = std::fs::metadata(at("d/same.txt")).unwrap();
        use std::os::unix::fs::MetadataExt;
        let file = |name: &str, size: u64, mtime: i64, nsec: u32| {
            let mut e = entry(&format!("{root}/d/{name}"), Kind::File);
            e.size = size;
            e.mtime = mtime;
            e.mtime_nsec = nsec;
            e
        };
        let entries = vec![
            entry(&format!("{root}/d"), Kind::Dir),
            file("changed.txt", 5, 1, 0),
            entry(&format!("{root}/d/new"), Kind::Dir),
            file("new/a.txt", 7, 1, 0),
            file("same.txt", 4, st.mtime(), st.mtime_nsec() as u32),
        ];
        let snap = test_snapshot();
        let req = RestoreRequest {
            snapshot: &snap,
            paths: vec![at("d")],
            to: None,
            keep_existing: true,
            dry_run: true,
            xattrs: XattrPolicy {
                acl: false,
                other: false,
            },
            links: LinkPolicy::FollowAll,
        };
        let p = preview(entries.into_iter().map(Ok), &req).unwrap();
        assert_eq!((p.new, p.differ, p.identical, p.new_folders), (1, 1, 1, 1));
        assert_eq!(p.bytes, 12);
        assert_eq!(p.differ_paths, vec![at("d/changed.txt")]);
        assert_eq!(p.new_paths, vec![at("d/new/a.txt")]);
        assert!(p.keep_existing);
        // Nothing was touched.
        assert_eq!(std::fs::read(at("d/changed.txt")).unwrap(), b"old");
        assert!(!at("d/new").exists());
    }
}
