//! Restore paths from a snapshot, to their original location or into another directory.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;
use serde::Serialize;

use flux_core::filter::Filter;
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
use flux_core::sync::Locked;

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
    /// Told before any file is written: the regular files this restore will ask for, in
    /// path order. A source on a server gets ready to fetch the small ones in batches.
    fn will_restore(&self, _files: &[&Entry]) {}
    /// Whether folder `e` is in the snapshot only as the way to something below it: its
    /// own permissions, owner and times were never backed up (Borg doesn't store the
    /// folders above the ones it's given). Such a folder is created when missing; one
    /// that is there is left exactly as it is.
    fn path_only(&self, _e: &Entry) -> bool {
        false
    }
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
    /// Make restored folders exactly as they are in the snapshot: what's in them now but
    /// not in the snapshot is removed (moved aside with `keep_existing`). Whatever the
    /// backup never takes, judged by this filter (exclusions, the size limit, cache
    /// folders), and anything on another filesystem, is left alone. `None`: only add and
    /// replace.
    pub exact: Option<&'a Filter>,
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
    /// Exact restores: items that weren't in the snapshot and were removed (or moved
    /// aside, then also listed in `moved_aside`).
    pub removed: u64,
    /// Exact restores: why some or all folders were left as they are (see
    /// [`exact_folders`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_notes: Vec<String>,
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

    let (items, unsafe_paths, above) = select(source.entries().map_err(io_err)?, req)?;

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
        errors.locked().push(fe);
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
    // Folders that are only the way to what's restored, and were there already: nothing
    // about them is known, so nothing about them is changed.
    let mut untouched: HashSet<&Path> = HashSet::new();
    for (e, dest) in items.iter().filter(|(e, _)| e.kind == Kind::Dir) {
        match ctx.make_dir(dest) {
            Ok(false) if source.path_only(e) => {
                untouched.insert(dest.as_path());
            }
            Ok(_) => {}
            Err(err) => warn(dest, &err),
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
    source.will_restore(
        &non_dirs
            .iter()
            .filter(|&&i| items[i].0.kind == Kind::File && follower_of[i].is_none())
            .map(|&i| &items[i].0)
            .collect::<Vec<_>>(),
    );
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

    // Exact: remove what the restored folders hold that the snapshot doesn't. Before the
    // folders' dates are set, which removing would change.
    let mut removed = 0u64;
    if let Some(filter) = req.exact {
        let wanted: HashSet<&Path> = items.iter().map(|(_, d)| d.as_path()).collect();
        let aside: HashSet<PathBuf> = moved.locked().iter().cloned().collect();
        let scope = exact_folders(&items, &above, req.snapshot);
        for note in &scope.notes {
            report(Event::Info {
                message: format!("exact restore: {note}"),
            });
        }
        out.exact_notes = scope.notes;
        for (e, dest) in scope.folders {
            let r = tree.dir(dest, false).and_then(|dir| {
                let found = list_fd(dir.as_fd())?;
                let parent_dev = rustix::fs::fstat(&*dir)?.st_dev;
                for f in extras(
                    found,
                    parent_dev,
                    (dest, &e.abs_path()),
                    filter,
                    &wanted,
                    &aside,
                    &scope.unread,
                ) {
                    let path = dest.join(&f.name);
                    let kind = if f.is_dir {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };
                    match ctx.set_aside(dir.as_fd(), &f.name, &path, kind) {
                        Ok(()) => removed += 1,
                        Err(err) => warn(&path, &err),
                    }
                }
                Ok(())
            });
            if let Err(err) = r {
                warn(dest, &err);
            }
        }
    }

    // Directory metadata last, deepest first, through descriptors of the directories.
    // (One of them per filesystem is kept, to sync it at the end.)
    let mut filesystems: HashMap<u64, rustix::fd::OwnedFd> = HashMap::new();
    for (e, dest) in items
        .iter()
        .rev()
        .filter(|(e, d)| e.kind == Kind::Dir && !untouched.contains(d.as_path()))
    {
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
            let done = apply_metadata_fd(fd.as_fd(), e, &policy, mode_mask);
            if let Ok(st) = rustix::fs::fstat(&fd) {
                filesystems.entry(st.st_dev).or_insert(fd);
            }
            done
        });
        if let Err(err) = r {
            warn(dest, &err);
        }
    }

    // What was restored is made durable once, here, for each filesystem it went to: not
    // file by file as it is written (on a spinning disk that is an fsync's wait per
    // file, hours for a million of them).
    if let Some((_, dest)) = items.first()
        && let Ok((dir, _)) = tree.parent(dest, false)
        && let Ok(fd) = rustix::fs::openat(
            &*dir,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        && let Ok(st) = rustix::fs::fstat(&fd)
    {
        filesystems.entry(st.st_dev).or_insert(fd);
    }
    for fd in filesystems.values() {
        let _ = rustix::fs::syncfs(fd);
    }

    progress.set_phase(Phase::Done);
    out.restored = restored.into_inner();
    out.skipped_identical = skipped.into_inner();
    out.bytes = bytes.into_inner();
    out.setid_dropped = setid_dropped.into_inner();
    out.errors = unsafe_paths;
    out.errors.extend(errors.into_value());
    out.moved_aside = moved.into_value();
    out.removed = removed;
    Ok(out)
}

/// An item found in a destination folder.
struct Found {
    name: OsString,
    is_dir: bool,
    size: u64,
    dev: u64,
}

/// What a folder at the destination holds, read through its descriptor (never following
/// a link).
fn list_fd(dir: BorrowedFd<'_>) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    // The tree's descriptors are for paths only; reading needs one opened for reading.
    let readable = rustix::fs::openat(
        dir,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut d = rustix::fs::Dir::read_from(&readable)?;
    while let Some(ent) = d.read() {
        let ent = ent?;
        let name = OsStr::from_bytes(ent.file_name().to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => out.push(Found {
                name: name.to_owned(),
                is_dir: FileType::from_raw_mode(st.st_mode) == FileType::Directory,
                size: st.st_size as u64,
                dev: st.st_dev,
            }),
            Err(Errno::NOENT) => {} // gone meanwhile
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Which of `found` (in destination folder `dest`, the snapshot's `original`) an exact
/// restore removes: what the snapshot doesn't have, unless the backup would never have
/// taken it (`filter`), it's on another filesystem, the backup couldn't read it
/// (`unread`), or it's a copy a restore moved aside.
fn extras(
    found: Vec<Found>,
    parent_dev: u64,
    (dest, original): (&Path, &Path),
    filter: &Filter,
    wanted: &HashSet<&Path>,
    aside: &HashSet<PathBuf>,
    unread: &[PathBuf],
) -> Vec<Found> {
    // A cache folder (or one with an exclude_if_present marker): its contents were never
    // backed up.
    if filter.has_markers()
        && filter.skip_dir_by_marker(dest, found.iter().map(|f| f.name.as_bytes()))
    {
        return Vec::new();
    }
    found
        .into_iter()
        .filter(|f| {
            let at = dest.join(&f.name);
            let was = original.join(&f.name);
            !wanted.contains(at.as_path())
                // What the backup couldn't read isn't in the snapshot, but it isn't extra.
                && !unread.iter().any(|u| was.starts_with(u))
                && !aside.contains(&at)
                // Copies an earlier restore kept (`name.flux-<snapshot>`): the person's own.
                && !f.name.as_bytes().windows(6).any(|w| w == b".flux-")
                && f.dev == parent_dev
                && !filter.is_excluded(&was, f.is_dir)
                && (f.is_dir || !filter.is_too_large(f.size))
        })
        .collect()
}

/// The entries a request covers, with their destinations, in path order; and the paths a
/// sound snapshot never contains (`..`, absolute…), which are never read or written.
type Selected = (Vec<(Entry, PathBuf)>, Vec<FileError>, Above);

/// The device of each folder above a requested path (an exact restore needs them to tell
/// a mount point the backup didn't go into).
type Above = HashMap<Vec<u8>, u64>;

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
    let mut above = Above::new();
    // Every link in the snapshot. A sound snapshot has nothing below one; a forged one
    // might, to have the restore make the link and then write through it.
    let mut links: HashSet<Vec<u8>> = HashSet::new();
    for e in entries {
        let e = e.map_err(io_err)?;
        if e.kind == Kind::Symlink {
            links.insert(e.path.clone());
        }
        if !is_clean_path(&e.path) {
            unsafe_paths.push(FileError {
                path: String::from_utf8_lossy(&e.path).into_owned(),
                message: "unsafe path in the snapshot (damaged or forged); skipped".into(),
            });
            continue;
        }
        if req.exact.is_some()
            && e.kind == Kind::Dir
            && wanted
                .iter()
                .any(|w| w.len() > e.path.len() && is_within(w, &e.path))
        {
            above.insert(e.path.clone(), e.dev);
        }
        // A request may name a folder and something inside it: every path that covers the
        // entry is found, and the outermost one decides where it goes.
        let mut outer: Option<&Vec<u8>> = None;
        for (i, w) in wanted.iter().enumerate() {
            if is_within(&e.path, w) {
                found[i] = true;
                if outer.is_none_or(|o| w.len() < o.len()) {
                    outer = Some(w);
                }
            }
        }
        if let Some(w) = outer {
            let dest = destination(&e, w, req.to.as_deref());
            items.push((e, dest));
        }
    }
    if let Some(i) = found.iter().position(|f| !f) {
        return Err(RestoreError::NotInSnapshot(
            req.paths[i].clone(),
            req.snapshot.id.clone(),
        ));
    }
    if !links.is_empty() {
        items.retain(|(e, _)| {
            let below_link = e
                .path
                .iter()
                .enumerate()
                .filter(|&(_, &b)| b == b'/')
                .any(|(i, _)| links.contains(&e.path[..i]));
            if below_link {
                unsafe_paths.push(FileError {
                    path: String::from_utf8_lossy(&e.path).into_owned(),
                    message: "listed below a link in the snapshot (damaged or forged); skipped"
                        .into(),
                });
            }
            !below_link
        });
    }
    Ok((items, unsafe_paths, above))
}

/// The folders among `items` an exact restore may remove extras from, the paths in them
/// it must leave alone, and why it holds back where it does.
///
/// Only what the backup really covered is authoritative:
/// - folders at or below one of the snapshot's sources (the folders above a source are
///   in the snapshot only to carry their metadata; what else they hold was never backed
///   up);
/// - not a mount point the backup didn't go into (another filesystem than its parent,
///   and nothing of it in the snapshot);
/// - not what the backup failed to read (the snapshot's errors). When the snapshot lists
///   only some of its errors, or doesn't say what it covered, nothing is removed at all.
struct ExactScope<'a> {
    folders: Vec<&'a (Entry, PathBuf)>,
    /// Original paths the backup couldn't read: they and what's below them stay.
    unread: Vec<PathBuf>,
    notes: Vec<String>,
}

fn exact_folders<'a>(
    items: &'a [(Entry, PathBuf)],
    above: &Above,
    snap: &SnapshotMeta,
) -> ExactScope<'a> {
    let mut scope = ExactScope {
        folders: Vec::new(),
        unread: snap.errors.iter().map(|e| PathBuf::from(&e.path)).collect(),
        notes: Vec::new(),
    };
    if !items.iter().any(|(e, _)| e.kind == Kind::Dir) {
        return scope;
    }
    if snap.sources.is_empty() {
        scope.notes.push(
            "nothing was removed: this snapshot doesn't record which folders it covered".into(),
        );
        return scope;
    }
    if snap.errors_total > snap.errors.len() as u64 {
        scope.notes.push(format!(
            "nothing was removed: the backup couldn't read {} items and the snapshot lists only {} of them",
            snap.errors_total,
            snap.errors.len()
        ));
        return scope;
    }
    let sources: Vec<Vec<u8>> = snap
        .sources
        .iter()
        .map(|p| {
            let b = p.as_os_str().as_bytes();
            let b = b.strip_prefix(b"/").unwrap_or(b);
            b.strip_suffix(b"/").unwrap_or(b).to_vec()
        })
        .collect();
    fn parent(p: &[u8]) -> Option<&[u8]> {
        p.iter().rposition(|&b| b == b'/').map(|i| &p[..i])
    }
    let mut dev: HashMap<&[u8], u64> = above.iter().map(|(p, d)| (p.as_slice(), *d)).collect();
    let mut has_children: HashSet<&[u8]> = HashSet::new();
    for (e, _) in items {
        if e.kind == Kind::Dir {
            dev.insert(&e.path, e.dev);
        }
        if let Some(p) = parent(&e.path) {
            has_children.insert(p);
        }
    }
    let (mut outside, mut mounts) = (0u64, 0u64);
    for item in items.iter().filter(|(e, _)| e.kind == Kind::Dir) {
        let e = &item.0;
        let Some(source) = sources.iter().find(|s| is_within(&e.path, s)) else {
            outside += 1;
            continue;
        };
        let other_fs = e.path.len() > source.len()
            && parent(&e.path)
                .and_then(|p| dev.get(p))
                .is_some_and(|d| *d != e.dev);
        if other_fs && !has_children.contains(e.path.as_slice()) {
            mounts += 1;
            continue;
        }
        scope.folders.push(item);
    }
    if outside > 0 {
        scope.notes.push(format!(
            "{outside} folder{} above what this backup covers: only what the backup holds was restored there, nothing was removed",
            if outside == 1 { " is" } else { "s are" }
        ));
    }
    if mounts > 0 {
        scope.notes.push(format!(
            "{mounts} folder{} another filesystem the backup didn't go into: left as {}",
            if mounts == 1 { " is" } else { "s are" },
            if mounts == 1 { "it is" } else { "they are" }
        ));
    }
    scope
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
    /// Exact restores: items not in the snapshot that would be removed (moved aside with
    /// `keep_existing`), and the first [`PREVIEW_PATHS`] of them.
    pub remove: u64,
    pub remove_paths: Vec<PathBuf>,
    /// Exact restores: why some or all folders would be left as they are.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exact_notes: Vec<String>,
}

/// What [`restore`] would do with `req`: compares the snapshot's entries with what is at
/// each destination now, by the same rule restore uses (a regular file with the same size
/// and modification time is identical). Reads nothing but metadata.
pub fn preview(
    entries: impl Iterator<Item = io::Result<Entry>>,
    req: &RestoreRequest<'_>,
) -> Result<RestorePreview, RestoreError> {
    use std::os::unix::fs::MetadataExt;
    let (items, unsafe_paths, above) = select(entries, req)?;
    let mut out = RestorePreview {
        keep_existing: req.keep_existing,
        unsafe_paths: unsafe_paths.len() as u64,
        ..Default::default()
    };
    if let Some(filter) = req.exact {
        let wanted: HashSet<&Path> = items.iter().map(|(_, d)| d.as_path()).collect();
        let scope = exact_folders(&items, &above, req.snapshot);
        out.exact_notes = scope.notes;
        for (e, dest) in scope.folders {
            // Only folders that are there now (and are folders) can hold anything extra.
            let Ok(m) = fs::symlink_metadata(dest) else {
                continue;
            };
            if !m.is_dir() {
                continue;
            }
            let Ok(rd) = fs::read_dir(dest) else { continue };
            let found = rd
                .flatten()
                .filter_map(|ent| {
                    let m = ent.metadata().ok()?; // DirEntry::metadata doesn't follow links
                    Some(Found {
                        name: ent.file_name(),
                        is_dir: m.is_dir(),
                        size: m.len(),
                        dev: m.dev(),
                    })
                })
                .collect();
            for f in extras(
                found,
                m.dev(),
                (dest, &e.abs_path()),
                filter,
                &wanted,
                &HashSet::new(),
                &scope.unread,
            ) {
                out.remove += 1;
                if out.remove_paths.len() < PREVIEW_PATHS {
                    out.remove_paths.push(dest.join(&f.name));
                }
            }
        }
        out.remove_paths.sort();
    }
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
    /// Make sure `dest` is a folder. Whether it had to be made (it wasn't there, or
    /// something else was in its place).
    fn make_dir(&self, dest: &Path) -> io::Result<bool> {
        let (dir, name) = self.tree.parent(dest, true)?;
        match rustix::fs::statat(&*dir, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Directory => {
                return Ok(false);
            }
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
            Ok(()) => Ok(true),
            Err(Errno::EXIST) => Ok(false),
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
        self.moved.locked().push(dest.with_file_name(&aside));
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
            sources_missing: Vec::new(),
            note: None,
            tags: Vec::new(),
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
                exact: None,
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

    /// A forged snapshot that lists a link and then files "below" it: the restore would
    /// make the link and then write through it, to wherever it points. Nothing below a
    /// link is restored, hard-linked files (restored after everything else) included.
    #[test]
    fn forged_entries_below_a_link_are_skipped() {
        struct Forged(Vec<Entry>);
        impl RestoreSource for Forged {
            fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<Entry>> + '_>> {
                Ok(Box::new(self.0.iter().cloned().map(Ok)))
            }
            fn write_file(
                &self,
                _e: &Entry,
                out: &File,
                _copier: &Copier,
                _progress: &Progress,
            ) -> io::Result<u64> {
                let mut out = out;
                std::io::Write::write_all(&mut out, b"data")?;
                Ok(4)
            }
        }
        let t = tempfile::tempdir().unwrap();
        let victim = t.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        let file = |path: &str, ino: u64| {
            let mut e = entry(path, Kind::File);
            e.size = 4;
            e.mode = 0o644;
            if ino != 0 {
                e.nlink = 2;
                e.dev = 7;
                e.ino = ino;
            }
            e
        };
        let mut dir = entry("src", Kind::Dir);
        dir.mode = 0o755;
        let mut link = entry("src/x", Kind::Symlink);
        link.link = Some(victim.as_os_str().as_encoded_bytes().to_vec());
        let source = Forged(vec![
            dir,
            file("src/a", 9),
            link,
            file("src/x/planted", 0),
            file("src/x/linked", 9),
            file("src/x/deep/er", 0),
        ]);
        let snap = test_snapshot();
        let to = t.path().join("dest");
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
                exact: None,
            },
            &Progress::new(),
            &flux_core::progress::silent,
        )
        .unwrap();
        assert_eq!(std::fs::read(to.join("src/a")).unwrap(), b"data");
        assert!(to.join("src/x").is_symlink(), "the link itself is restored");
        assert_eq!(
            std::fs::read_dir(&victim).unwrap().count(),
            0,
            "nothing was written where the link points"
        );
        assert_eq!(out.errors.len(), 3, "{:?}", out.errors);
        assert!(
            out.errors
                .iter()
                .all(|e| e.message.contains("below a link"))
        );
    }

    /// Entries with their destination under `to` (as a restore of `/` into `to` gives).
    fn items_under(entries: Vec<Entry>) -> Vec<(Entry, PathBuf)> {
        entries
            .into_iter()
            .map(|e| {
                let d = e.abs_path();
                (e, d)
            })
            .collect()
    }

    fn dir(path: &str, dev: u64) -> Entry {
        let mut e = entry(path, Kind::Dir);
        e.dev = dev;
        e
    }

    /// Only folders the backup covered are made exact: not the folders above a source,
    /// not a mount point it didn't go into, and nothing when the snapshot doesn't say.
    #[test]
    fn exact_only_where_the_backup_covered() {
        let mut snap = test_snapshot();
        snap.sources = vec!["/home/me/Documents".into(), "/home/me/Pictures/".into()];
        let items = items_under(vec![
            dir("home", 1),
            dir("home/me", 1),
            dir("home/me/Documents", 1),
            entry("home/me/Documents/a.txt", Kind::File),
            dir("home/me/Documents/mnt", 2), // another filesystem, nothing of it kept
            dir("home/me/Documents/nas", 3), // another filesystem the backup went into
            entry("home/me/Documents/nas/b.txt", Kind::File),
            dir("home/me/Pictures", 1),
        ]);
        let scope = exact_folders(&items, &Above::new(), &snap);
        let got: Vec<_> = scope
            .folders
            .iter()
            .map(|(e, _)| String::from_utf8_lossy(&e.path).into_owned())
            .collect();
        assert_eq!(
            got,
            [
                "home/me/Documents",
                "home/me/Documents/nas",
                "home/me/Pictures"
            ]
        );
        assert_eq!(scope.notes.len(), 2, "{:?}", scope.notes);

        // The mount point itself is what's restored: its parent's device comes from above.
        let only = items_under(vec![dir("home/me/Documents/mnt", 2)]);
        let above: Above = [(b"home/me/Documents".to_vec(), 1)].into();
        assert!(exact_folders(&only, &above, &snap).folders.is_empty());
        // A source that is itself a mount point is covered.
        let src = items_under(vec![dir("home/me/Pictures", 7)]);
        let above: Above = [(b"home/me".to_vec(), 1)].into();
        assert_eq!(exact_folders(&src, &above, &snap).folders.len(), 1);

        // More errors than the snapshot lists, or no sources recorded: nothing at all.
        snap.errors_total = 3;
        let scope = exact_folders(&items, &Above::new(), &snap);
        assert!(scope.folders.is_empty() && scope.notes.len() == 1);
        snap.errors_total = 0;
        snap.sources.clear();
        let scope = exact_folders(&items, &Above::new(), &snap);
        assert!(scope.folders.is_empty() && scope.notes.len() == 1);
    }

    /// An exact restore of the folder above a source leaves what else is in it, and what
    /// the backup couldn't read, alone; a real extra inside the source goes.
    #[test]
    fn exact_restore_leaves_what_was_never_covered() {
        struct Dirs(Vec<Entry>);
        impl RestoreSource for Dirs {
            fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<Entry>> + '_>> {
                Ok(Box::new(self.0.iter().cloned().map(Ok)))
            }
            fn write_file(
                &self,
                _e: &Entry,
                _out: &File,
                _copier: &Copier,
                _progress: &Progress,
            ) -> io::Result<u64> {
                Ok(0)
            }
        }
        use std::os::unix::fs::MetadataExt;
        let t = tempfile::tempdir().unwrap();
        let at = |rel: &str| t.path().join(rel);
        for d in ["user/Documents", "user/.ssh", "user/Photos"] {
            std::fs::create_dir_all(at(d)).unwrap();
        }
        std::fs::write(at("user/notes.txt"), "mine").unwrap();
        std::fs::write(at("user/Documents/extra.txt"), "x").unwrap();
        std::fs::write(at("user/Documents/unreadable.db"), "u").unwrap();
        let rel = |p: &str| {
            at(p)
                .strip_prefix("/")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        };
        let d = |p: &str| {
            let m = std::fs::metadata(at(p)).unwrap();
            let mut e = entry(&rel(p), Kind::Dir);
            (e.mode, e.uid, e.gid, e.dev) = (m.mode() & 0o7777, m.uid(), m.gid(), m.dev());
            e
        };
        let source = Dirs(vec![d("user"), d("user/Documents")]);
        let mut snap = test_snapshot();
        snap.sources = vec![at("user/Documents")];
        snap.errors = vec![FileError {
            path: at("user/Documents/unreadable.db").display().to_string(),
            message: "Permission denied".into(),
        }];
        snap.errors_total = 1;
        let mut profile = flux_core::Profile::new(
            "t",
            flux_core::profile::Target::Local {
                path: at("target"),
                fs_uuid: None,
            },
            vec![at("user/Documents")],
        );
        profile.sources.exclude.clear();
        let filter = Filter::new(&profile.sources, None).unwrap();
        let req = |dry_run| RestoreRequest {
            snapshot: &snap,
            paths: vec![at("user")],
            to: None,
            keep_existing: false,
            dry_run,
            xattrs: XattrPolicy {
                acl: false,
                other: false,
            },
            links: LinkPolicy::FollowAll,
            exact: Some(&filter),
        };
        let p = preview(source.0.iter().cloned().map(Ok), &req(true)).unwrap();
        assert_eq!(p.remove_paths, vec![at("user/Documents/extra.txt")]);
        assert_eq!(p.exact_notes.len(), 1, "{:?}", p.exact_notes);

        let out = restore(&source, &req(false), &Progress::default(), &|_| {}).unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.removed, 1);
        assert!(!at("user/Documents/extra.txt").exists());
        for kept in [
            "user/.ssh",
            "user/Photos",
            "user/notes.txt",
            "user/Documents/unreadable.db",
        ] {
            assert!(at(kept).exists(), "{kept} was never in the backup");
        }
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
            exact: None,
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
