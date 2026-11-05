//! Serialized structures of the repository format.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_fs::entry::{Entry, Kind, Xattr};
use flux_fs::snapshot::SnapshotMeta;

use crate::crypto::BlobId;

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkerParams {
    pub min: u32,
    pub avg: u32,
    pub max: u32,
}

impl ChunkerParams {
    pub const DEFAULT: ChunkerParams = ChunkerParams {
        min: 512 << 10,
        avg: 1 << 20,
        max: 8 << 20,
    };
}

/// Keys stored in the clear in unencrypted repositories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlainKeys {
    pub id_key: String,
    pub chunk_seed: u64,
}

/// `config`: plaintext repository description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub format: u32,
    /// Random 16-byte id (hex); binds key slots to this repository.
    pub id: String,
    pub encrypted: bool,
    pub created: Timestamp,
    pub chunker: ChunkerParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plain_keys: Option<PlainKeys>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobKind {
    Data,
    Tree,
}

/// One blob inside a pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackEntry {
    pub kind: BlobKind,
    pub id: BlobId,
    pub offset: u32,
    pub length: u32,
    pub raw_length: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPack {
    pub pack: BlobId,
    pub blobs: Vec<PackEntry>,
}

/// `index/<id>` payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexFile {
    pub packs: Vec<IndexPack>,
    /// Index files this one replaces (written by prune).
    pub supersedes: Vec<String>,
}

/// A directory entry in a tree blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    #[serde(with = "serde_bytes")]
    pub name: Vec<u8>,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: u32,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub rdev: u64,
    #[serde(with = "serde_bytes")]
    pub link: Option<Vec<u8>>,
    pub hash: Option<[u8; 32]>,
    pub xattrs: Vec<Xattr>,
    /// Files: data blobs, in order.
    pub content: Vec<BlobId>,
    /// Directories: the tree blob of the contents.
    pub subtree: Option<BlobId>,
}

impl Node {
    pub fn from_entry(e: &Entry, name: Vec<u8>) -> Self {
        Node {
            name,
            kind: e.kind,
            mode: e.mode,
            uid: e.uid,
            gid: e.gid,
            size: e.size,
            mtime: e.mtime,
            mtime_nsec: e.mtime_nsec,
            dev: e.dev,
            ino: e.ino,
            nlink: e.nlink,
            rdev: e.rdev,
            link: e.link.clone(),
            hash: e.hash,
            xattrs: e.xattrs.clone(),
            content: Vec::new(),
            subtree: None,
        }
    }

    /// The entry for this node at full path `path` (relative, no leading `/`).
    pub fn to_entry(&self, path: Vec<u8>) -> Entry {
        Entry {
            path,
            kind: self.kind,
            mode: self.mode,
            uid: self.uid,
            gid: self.gid,
            size: self.size,
            mtime: self.mtime,
            mtime_nsec: self.mtime_nsec,
            dev: self.dev,
            ino: self.ino,
            nlink: self.nlink,
            rdev: self.rdev,
            link: self.link.clone(),
            hash: self.hash,
            xattrs: self.xattrs.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

/// `snapshots/<id>` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub meta: SnapshotMeta,
    /// Root tree id (hex).
    pub tree: String,
    pub parent: Option<String>,
}
