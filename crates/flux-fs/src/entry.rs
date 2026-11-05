//! Metadata of one backed-up path, shared by every engine.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dir,
    File,
    Symlink,
    Fifo,
    CharDevice,
    BlockDevice,
    Socket,
}

impl Kind {
    pub fn from_file_type(ft: std::fs::FileType) -> Self {
        use std::os::unix::fs::FileTypeExt;
        if ft.is_dir() {
            Kind::Dir
        } else if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_fifo() {
            Kind::Fifo
        } else if ft.is_char_device() {
            Kind::CharDevice
        } else if ft.is_block_device() {
            Kind::BlockDevice
        } else if ft.is_socket() {
            Kind::Socket
        } else {
            Kind::File
        }
    }

    /// One-letter code as in `ls -l`.
    pub fn letter(self) -> char {
        match self {
            Kind::Dir => 'd',
            Kind::File => '-',
            Kind::Symlink => 'l',
            Kind::Fifo => 'p',
            Kind::CharDevice => 'c',
            Kind::BlockDevice => 'b',
            Kind::Socket => 's',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Xattr {
    #[serde(with = "serde_bytes")]
    pub name: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
}

/// Metadata of one backed-up path, as it was on the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Absolute source path without the leading `/` (e.g. `home/me/notes.txt`). Raw bytes,
    /// because Linux file names need not be UTF-8.
    #[serde(with = "serde_bytes")]
    pub path: Vec<u8>,
    pub kind: Kind,
    /// Permission bits including setuid/setgid/sticky (`st_mode & 0o7777`).
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: u32,
    /// Source device and inode, used to recreate hard links within a snapshot.
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    /// Device number for character/block devices.
    pub rdev: u64,
    /// Symlink target.
    #[serde(with = "serde_bytes")]
    pub link: Option<Vec<u8>>,
    /// BLAKE3 of the contents, when known (checksum mode, or carried over).
    pub hash: Option<[u8; 32]>,
    pub xattrs: Vec<Xattr>,
}

impl Entry {
    pub fn rel_path(&self) -> &Path {
        Path::new(OsStr::from_bytes(&self.path))
    }

    /// The original absolute path.
    pub fn abs_path(&self) -> PathBuf {
        Path::new("/").join(self.rel_path())
    }

    /// Same content and same metadata that lives on the inode, so a hard link to the
    /// previous snapshot's copy is a faithful backup of this file.
    pub fn same_file_as(&self, prev: &Entry) -> bool {
        self.kind == prev.kind
            && self.size == prev.size
            && self.mtime == prev.mtime
            && self.mtime_nsec == prev.mtime_nsec
            && self.mode == prev.mode
            && self.uid == prev.uid
            && self.gid == prev.gid
            && self.xattrs == prev.xattrs
            && match (&self.hash, &prev.hash) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }

    /// Same metadata, for non-regular entries. Directory mtimes are ignored: they change
    /// whenever a child is added or removed, which shows up as its own change anyway.
    pub fn same_meta_as(&self, prev: &Entry) -> bool {
        self.kind == prev.kind
            && self.mode == prev.mode
            && self.uid == prev.uid
            && self.gid == prev.gid
            && self.link == prev.link
            && self.rdev == prev.rdev
            && self.xattrs == prev.xattrs
            && (self.kind == Kind::Dir
                || (self.mtime == prev.mtime && self.mtime_nsec == prev.mtime_nsec))
    }
}

/// Order used for manifests and plans: plain byte order of the relative path. A directory
/// always sorts before its descendants, because it is a prefix of their paths.
pub fn path_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.cmp(b)
}

/// Is `path` equal to `prefix` or below it? (`prefix` = `b""` means everything.)
/// Is `path` a plain relative path, safe to join onto a directory? Snapshot paths always
/// are; a damaged or forged manifest or tree might not be. Rejects absolute paths, empty
/// components, `.`, `..` and NUL bytes.
pub fn is_clean_path(path: &[u8]) -> bool {
    !path.is_empty()
        && !path.contains(&0)
        && path
            .split(|&b| b == b'/')
            .all(|c| !c.is_empty() && c != b"." && c != b"..")
}

pub fn is_within(path: &[u8], prefix: &[u8]) -> bool {
    prefix.is_empty()
        || (path.starts_with(prefix) && (path.len() == prefix.len() || path[prefix.len()] == b'/'))
}

/// An entry with plain defaults, for tests in this and other crates.
#[doc(hidden)]
pub fn test_entry(path: &str, kind: Kind) -> Entry {
    Entry {
        path: path.as_bytes().to_vec(),
        kind,
        mode: 0o644,
        uid: 1000,
        gid: 1000,
        size: 0,
        mtime: 1_700_000_000,
        mtime_nsec: 0,
        dev: 1,
        ino: 1,
        nlink: 1,
        rdev: 0,
        link: None,
        hash: None,
        xattrs: vec![],
    }
}
