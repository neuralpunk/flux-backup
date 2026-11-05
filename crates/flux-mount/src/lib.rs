//! Snapshots as a read-only filesystem (FUSE), for every engine: `flux mount`.
//!
//! ```text
//! <mountpoint>/
//!   latest -> 2026-11-05_10-00-00
//!   2026-11-04_10-00-00/home/me/…
//!   2026-11-05_10-00-00/home/me/…
//! ```
//!
//! Each snapshot is a folder named after its start time (local time), holding the backed-up
//! paths as they were. With `only`, the mount point shows one snapshot directly. A
//! snapshot's listing is loaded when it is first opened, and a few are kept in memory.
//!
//! Inode numbers are derived from positions, so they're stable while mounted: `1` is the
//! root, `2` is `latest`, snapshot `k` is `(k+1) << 40` and its entry `i` is that plus
//! `i + 1`. The mount is read-only, `nosuid` and `nodev`, and permissions are checked by
//! the kernel against the stored owners and modes (`default_permissions`).

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyOpen, Request,
};

use flux_fs::entry::{Entry, Kind};
use flux_fs::snapshot::SnapshotMeta;
use flux_runner::repo::AnyRepo;
use flux_runner::view::{FileReader, SnapshotView};

pub use fuser::BackgroundSession;

const SHIFT: u32 = 40;
const ROOT: u64 = 1;
const LATEST: u64 = 2;
/// Snapshot listings kept in memory.
const KEEP_VIEWS: usize = 6;
/// Everything is immutable, so the kernel may cache it for long.
const TTL: Duration = Duration::from_secs(3600);

/// What an inode number refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    Root,
    Latest,
    /// The top folder of snapshot `k`.
    Snapshot(usize),
    /// Entry `i` of snapshot `k`.
    Entry(usize, usize),
}

/// A loaded snapshot: its view plus, for each directory, its children sorted by name.
struct Loaded {
    view: SnapshotView,
    /// `children[i]`: entries directly inside entry `i`; the last element holds the
    /// snapshot's top level.
    children: Vec<Vec<u32>>,
}

impl Loaded {
    fn new(view: SnapshotView) -> Self {
        let n = view.entries.len();
        let mut by_path: HashMap<&[u8], u32> = HashMap::with_capacity(n);
        for (i, e) in view.entries.iter().enumerate() {
            by_path.insert(&e.path, i as u32);
        }
        let mut children = vec![Vec::new(); n + 1];
        for (i, e) in view.entries.iter().enumerate() {
            // A damaged or forged snapshot's `..`-style paths are simply not shown.
            if !flux_fs::entry::is_clean_path(&e.path) {
                continue;
            }
            let parent = match e.path.iter().rposition(|&b| b == b'/') {
                None => n,
                Some(p) => match by_path.get(&e.path[..p]) {
                    Some(&pi) => pi as usize,
                    None => continue, // orphan: its parent wasn't stored
                },
            };
            children[parent].push(i as u32);
        }
        for c in &mut children {
            c.sort_by(|&a, &b| {
                name(&view.entries[a as usize]).cmp(name(&view.entries[b as usize]))
            });
        }
        drop(by_path);
        Self { view, children }
    }

    fn top(&self) -> usize {
        self.view.entries.len()
    }

    fn find(&self, dir: usize, wanted: &[u8]) -> Option<usize> {
        let c = &self.children[dir];
        c.binary_search_by(|&i| name(&self.view.entries[i as usize]).cmp(wanted))
            .ok()
            .map(|p| c[p] as usize)
    }
}

/// Last path component of an entry.
fn name(e: &Entry) -> &[u8] {
    match e.path.iter().rposition(|&b| b == b'/') {
        Some(p) => &e.path[p + 1..],
        None => &e.path,
    }
}

fn file_type(k: Kind) -> FileType {
    match k {
        Kind::Dir => FileType::Directory,
        Kind::File => FileType::RegularFile,
        Kind::Symlink => FileType::Symlink,
        Kind::Fifo => FileType::NamedPipe,
        Kind::CharDevice => FileType::CharDevice,
        Kind::BlockDevice => FileType::BlockDevice,
        Kind::Socket => FileType::Socket,
    }
}

fn time(sec: i64, nsec: u32) -> SystemTime {
    if sec >= 0 {
        UNIX_EPOCH + Duration::new(sec as u64, nsec)
    } else {
        UNIX_EPOCH - Duration::from_secs(sec.unsigned_abs())
    }
}

/// Folder names for snapshots: local start time, made unique with the id when needed.
pub fn snapshot_names(snaps: &[SnapshotMeta]) -> Vec<String> {
    let tz = jiff_tz();
    let base: Vec<String> = snaps
        .iter()
        .map(|s| {
            s.started
                .to_zoned(tz.clone())
                .strftime("%Y-%m-%d_%H-%M-%S")
                .to_string()
        })
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for b in &base {
        *seen.entry(b).or_default() += 1;
    }
    base.iter()
        .zip(snaps)
        .map(|(b, s)| {
            if seen[b.as_str()] > 1 {
                format!("{b}_{}", &s.id[..s.id.len().min(8)])
            } else {
                b.clone()
            }
        })
        .collect()
}

fn jiff_tz() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::system()
}

/// An open file, shared by the threads serving reads.
type Handle = Arc<Mutex<Box<dyn FileReader>>>;
/// (name, value)
type XattrPair = (Vec<u8>, Vec<u8>);

pub struct SnapshotFs {
    repo: Arc<AnyRepo>,
    snapshots: Vec<SnapshotMeta>,
    names: Vec<String>,
    by_name: HashMap<String, usize>,
    /// Show only this snapshot, at the root.
    only: Option<usize>,
    views: Mutex<VecDeque<(usize, Arc<Loaded>)>>,
    handles: Mutex<HashMap<u64, Handle>>,
    next_fh: AtomicU64,
    uid: u32,
    gid: u32,
}

impl SnapshotFs {
    /// `snapshots` oldest first; `only` restricts the mount to one of them.
    pub fn new(repo: Arc<AnyRepo>, snapshots: Vec<SnapshotMeta>, only: Option<usize>) -> Self {
        let names = snapshot_names(&snapshots);
        let by_name = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        Self {
            repo,
            snapshots,
            names,
            by_name,
            only,
            views: Mutex::new(VecDeque::new()),
            handles: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        }
    }

    fn node(&self, ino: u64) -> Option<Node> {
        match ino {
            ROOT => Some(match self.only {
                Some(k) => Node::Snapshot(k),
                None => Node::Root,
            }),
            LATEST if self.only.is_none() && !self.snapshots.is_empty() => Some(Node::Latest),
            _ => {
                let k = (ino >> SHIFT).checked_sub(1)? as usize;
                if k >= self.snapshots.len() || self.only.is_some_and(|o| o != k) {
                    return None;
                }
                match ino & ((1 << SHIFT) - 1) {
                    0 => Some(Node::Snapshot(k)),
                    i => Some(Node::Entry(k, i as usize - 1)),
                }
            }
        }
    }

    fn ino(&self, n: Node) -> u64 {
        match n {
            Node::Root => ROOT,
            Node::Latest => LATEST,
            Node::Snapshot(k) if self.only == Some(k) => ROOT,
            Node::Snapshot(k) => (k as u64 + 1) << SHIFT,
            Node::Entry(k, i) => ((k as u64 + 1) << SHIFT) | (i as u64 + 1),
        }
    }

    fn loaded(&self, k: usize) -> Result<Arc<Loaded>, Errno> {
        if let Some(l) = {
            let mut v = self.views.lock().expect("views");
            v.iter().position(|(n, _)| *n == k).map(|p| {
                let item = v.remove(p).expect("found");
                let l = item.1.clone();
                v.push_front(item);
                l
            })
        } {
            return Ok(l);
        }
        let view = SnapshotView::load(&self.repo, self.snapshots[k].clone()).map_err(|e| {
            tracing::warn!("cannot read snapshot {}: {e}", self.snapshots[k].id);
            Errno::EIO
        })?;
        let l = Arc::new(Loaded::new(view));
        let mut v = self.views.lock().expect("views");
        v.push_front((k, l.clone()));
        v.truncate(KEEP_VIEWS);
        Ok(l)
    }

    fn dir_attr(&self, ino: u64, t: SystemTime) -> FileAttr {
        FileAttr {
            ino: INodeNo(ino),
            size: 0,
            blocks: 0,
            atime: t,
            mtime: t,
            ctime: t,
            crtime: t,
            kind: FileType::Directory,
            perm: 0o555,
            nlink: 2,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn started(&self, k: usize) -> SystemTime {
        let t = self.snapshots[k].started;
        time(t.as_second(), t.subsec_nanosecond().max(0) as u32)
    }

    fn attr(&self, n: Node) -> Result<FileAttr, Errno> {
        Ok(match n {
            Node::Root => self.dir_attr(
                ROOT,
                self.snapshots
                    .len()
                    .checked_sub(1)
                    .map_or(UNIX_EPOCH, |k| self.started(k)),
            ),
            Node::Latest => {
                let target = self.names.last().map_or(0, String::len);
                let mut a = self.dir_attr(LATEST, self.started(self.snapshots.len() - 1));
                a.kind = FileType::Symlink;
                a.perm = 0o777;
                a.nlink = 1;
                a.size = target as u64;
                a
            }
            Node::Snapshot(k) => self.dir_attr(self.ino(n), self.started(k)),
            Node::Entry(k, i) => {
                let l = self.loaded(k)?;
                let e = l.view.entries.get(i).ok_or(Errno::ENOENT)?;
                let t = time(e.mtime, e.mtime_nsec);
                let size = match e.kind {
                    Kind::File => e.size,
                    Kind::Symlink => e.link.as_ref().map_or(0, |l| l.len() as u64),
                    _ => 0,
                };
                FileAttr {
                    ino: INodeNo(self.ino(n)),
                    size,
                    blocks: size.div_ceil(512),
                    atime: t,
                    mtime: t,
                    ctime: t,
                    crtime: t,
                    kind: file_type(e.kind),
                    perm: (e.mode & 0o7777) as u16,
                    nlink: if e.kind == Kind::Dir {
                        2
                    } else {
                        e.nlink.clamp(1, u64::from(u32::MAX)) as u32
                    },
                    uid: e.uid,
                    gid: e.gid,
                    rdev: e.rdev as u32,
                    blksize: 4096,
                    flags: 0,
                }
            }
        })
    }

    /// The stored extended attributes (ACLs, labels…) of an inode, as (name, value).
    fn xattrs(&self, ino: u64) -> Result<Vec<XattrPair>, Errno> {
        match self.node(ino).ok_or(Errno::ENOENT)? {
            Node::Entry(k, i) => {
                let l = self.loaded(k)?;
                let e = l.view.entries.get(i).ok_or(Errno::ENOENT)?;
                Ok(e.xattrs
                    .iter()
                    .map(|x| (x.name.to_vec(), x.value.to_vec()))
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    /// A directory's contents: (node, type, name).
    fn list(&self, n: Node) -> Result<Vec<(Node, FileType, Vec<u8>)>, Errno> {
        match n {
            Node::Root => {
                let mut out = Vec::with_capacity(self.snapshots.len() + 1);
                if !self.snapshots.is_empty() {
                    out.push((Node::Latest, FileType::Symlink, b"latest".to_vec()));
                }
                for (k, name) in self.names.iter().enumerate() {
                    out.push((
                        Node::Snapshot(k),
                        FileType::Directory,
                        name.as_bytes().to_vec(),
                    ));
                }
                Ok(out)
            }
            Node::Snapshot(k) | Node::Entry(k, _) => {
                let l = self.loaded(k)?;
                let dir = match n {
                    Node::Entry(_, i) => {
                        if l.view.entries.get(i).ok_or(Errno::ENOENT)?.kind != Kind::Dir {
                            return Err(Errno::ENOTDIR);
                        }
                        i
                    }
                    _ => l.top(),
                };
                Ok(l.children[dir]
                    .iter()
                    .map(|&c| {
                        let e = &l.view.entries[c as usize];
                        (
                            Node::Entry(k, c as usize),
                            file_type(e.kind),
                            name(e).to_vec(),
                        )
                    })
                    .collect())
            }
            Node::Latest => Err(Errno::ENOTDIR),
        }
    }

    fn lookup_node(&self, parent: Node, wanted: &[u8]) -> Result<Node, Errno> {
        match parent {
            Node::Root => {
                if wanted == b"latest" && !self.snapshots.is_empty() {
                    return Ok(Node::Latest);
                }
                std::str::from_utf8(wanted)
                    .ok()
                    .and_then(|s| self.by_name.get(s))
                    .map(|&k| Node::Snapshot(k))
                    .ok_or(Errno::ENOENT)
            }
            Node::Snapshot(k) | Node::Entry(k, _) => {
                let l = self.loaded(k)?;
                let dir = match parent {
                    Node::Entry(_, i) => i,
                    _ => l.top(),
                };
                if dir < l.top() && l.view.entries[dir].kind != Kind::Dir {
                    return Err(Errno::ENOTDIR);
                }
                l.find(dir, wanted)
                    .map(|i| Node::Entry(k, i))
                    .ok_or(Errno::ENOENT)
            }
            Node::Latest => Err(Errno::ENOTDIR),
        }
    }
}

impl Filesystem for SnapshotFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let r = self
            .node(parent.0)
            .ok_or(Errno::ENOENT)
            .and_then(|p| self.lookup_node(p, name.as_bytes()))
            .and_then(|n| self.attr(n));
        match r {
            Ok(a) => reply.entry(&TTL, &a, Generation(0)),
            Err(e) => reply.error(e),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self
            .node(ino.0)
            .ok_or(Errno::ENOENT)
            .and_then(|n| self.attr(n))
        {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(e),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.node(ino.0) {
            Some(Node::Latest) => reply.data(self.names.last().map_or(&b""[..], |s| s.as_bytes())),
            Some(Node::Entry(k, i)) => match self.loaded(k) {
                Ok(l) => match l.view.entries.get(i).and_then(|e| e.link.as_ref()) {
                    Some(t) => reply.data(t),
                    None => reply.error(Errno::EINVAL),
                },
                Err(e) => reply.error(e),
            },
            _ => reply.error(Errno::EINVAL),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.0 & (libc_flags::O_WRONLY | libc_flags::O_RDWR) != 0 {
            return reply.error(Errno::EROFS);
        }
        let Some(Node::Entry(k, i)) = self.node(ino.0) else {
            return reply.error(Errno::EISDIR);
        };
        let r = self
            .loaded(k)
            .and_then(|l| l.view.open(i).map_err(Errno::from));
        match r {
            Ok(f) => {
                let fh = self.next_fh.fetch_add(1, Relaxed);
                self.handles
                    .lock()
                    .expect("handles")
                    .insert(fh, Arc::new(Mutex::new(f)));
                reply.opened(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE);
            }
            Err(e) => reply.error(e),
        }
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: fuser::LockOwner,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }

    fn listxattr(&self, _req: &Request, ino: INodeNo, size: u32, reply: fuser::ReplyXattr) {
        let names = match self.xattrs(ino.0) {
            Ok(x) => x.iter().fold(Vec::new(), |mut v, (n, _)| {
                v.extend_from_slice(n);
                v.push(0);
                v
            }),
            Err(e) => return reply.error(e),
        };
        xattr_reply(reply, size, &names);
    }

    fn getxattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        name: &OsStr,
        size: u32,
        reply: fuser::ReplyXattr,
    ) {
        match self.xattrs(ino.0) {
            Ok(x) => match x.iter().find(|(n, _)| n.as_slice() == name.as_bytes()) {
                Some((_, v)) => xattr_reply(reply, size, v),
                None => reply.error(Errno::NO_XATTR),
            },
            Err(e) => reply.error(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let h = self.handles.lock().expect("handles").get(&fh.0).cloned();
        let Some(h) = h else {
            return reply.error(Errno::EBADF);
        };
        let r = h.lock().expect("file").read_at(offset, size as usize);
        match r {
            Ok(data) => reply.data(&data),
            Err(e) => {
                tracing::warn!("read failed: {e}");
                reply.error(Errno::EIO)
            }
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        self.handles.lock().expect("handles").remove(&fh.0);
        reply.ok();
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(n) = self.node(ino.0) else {
            return reply.error(Errno::ENOENT);
        };
        let items = match self.list(n) {
            Ok(i) => i,
            Err(e) => return reply.error(e),
        };
        let dots = [
            (n, FileType::Directory, b".".to_vec()),
            (n, FileType::Directory, b"..".to_vec()),
        ];
        for (pos, (node, kind, name)) in dots
            .into_iter()
            .chain(items)
            .enumerate()
            .skip(offset as usize)
        {
            if reply.add(
                INodeNo(self.ino(node)),
                pos as u64 + 1,
                kind,
                OsStr::from_bytes(&name),
            ) {
                break;
            }
        }
        reply.ok();
    }
}

/// Answer an xattr request: the size when asked for it (`size == 0`), else the data.
fn xattr_reply(reply: fuser::ReplyXattr, size: u32, data: &[u8]) {
    if size == 0 {
        reply.size(data.len() as u32);
    } else if (size as usize) < data.len() {
        reply.error(Errno::ERANGE);
    } else {
        reply.data(data);
    }
}

/// Open-flag bits (Linux values), to refuse writes.
mod libc_flags {
    pub const O_WRONLY: i32 = 0o1;
    pub const O_RDWR: i32 = 0o2;
}

/// Mount `fs` at `mountpoint` in the background. Dropping or `umount_and_join`ing the
/// session unmounts it.
pub fn mount(fs: SnapshotFs, mountpoint: &Path) -> std::io::Result<BackgroundSession> {
    let mut config = fuser::Config::default();
    config.mount_options = vec![
        MountOption::RO,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::DefaultPermissions,
        MountOption::FSName("flux".into()),
        MountOption::Subtype("flux".into()),
    ];
    config.n_threads = Some(4);
    fuser::spawn_mount(fs, mountpoint, &config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_fs::entry::test_entry;

    #[test]
    fn snapshot_folder_names() {
        let mk = |id: &str, t: &str| SnapshotMeta {
            id: id.into(),
            started: t.parse().unwrap(),
            ..test_meta()
        };
        let s = vec![
            mk("aaaaaaaaaa", "2026-11-05T10:00:00Z"),
            mk("bbbbbbbbbb", "2026-11-05T10:00:00Z"),
            mk("cccccccccc", "2026-11-06T10:00:00Z"),
        ];
        let n = snapshot_names(&s);
        assert_eq!(n.len(), 3);
        assert!(
            n[0].ends_with("_aaaaaaaa") && n[1].ends_with("_bbbbbbbb"),
            "{n:?}"
        );
        assert!(!n[2].contains("_cccc"), "{n:?}");

        assert_eq!(
            name(&test_entry("home/me/notes.md", Kind::File)),
            b"notes.md"
        );
        assert_eq!(name(&test_entry("home", Kind::Dir)), b"home");
    }

    fn test_meta() -> SnapshotMeta {
        SnapshotMeta {
            format: 1,
            id: "x".into(),
            profile: "p".into(),
            host: "h".into(),
            user: "u".into(),
            started: jiff::Timestamp::UNIX_EPOCH,
            finished: jiff::Timestamp::UNIX_EPOCH,
            status: flux_fs::snapshot::SnapshotStatus::Complete,
            name: None,
            pinned: false,
            sources: Vec::new(),
            stats: Default::default(),
            errors: Vec::new(),
            errors_total: 0,
            flux_version: "0".into(),
        }
    }
}
