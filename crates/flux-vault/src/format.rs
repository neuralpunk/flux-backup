//! Serialized structures of the repository format.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_fs::entry::{Entry, Kind, Xattr};
use flux_fs::snapshot::SnapshotMeta;

use crate::crypto::BlobId;

/// The newest repository format this build reads and writes, and the one new Vaults are
/// made in. Format 1 (Flux 0.1–0.3) is read, and written as format 1 until upgraded
/// (`flux vault upgrade`), so computers still on an older Flux can keep using a shared
/// Vault. Format 2 adds holes in sparse files. Format 3 (1.3) seals objects and key slots
/// with XAES-256-GCM instead of XChaCha20-Poly1305; its trees are format 2's.
pub const FORMAT: u32 = 3;

/// The newest tree format (snapshot records' `tree_format`), and the newest format an
/// older Vault is upgraded to: a change of `config` can't change how objects are sealed.
pub const TREE_FORMAT: u32 = 2;

/// The oldest repository format still read and written.
pub const OLDEST_FORMAT: u32 = 1;

/// A run of a sparse file that holds no data (reads as zeros), at a file offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hole {
    pub offset: u64,
    pub len: u64,
}

/// A file's stored content: its data, cut into blobs, with the holes left out. The file is
/// the data laid into the gaps between the holes, in order. No holes: the data is the file.
/// Holes sorted, not overlapping, not empty, and within `size`.
pub fn holes_are_sound(holes: &[Hole], size: u64) -> bool {
    let mut pos = 0u64;
    for h in holes {
        let Some(end) = h.offset.checked_add(h.len) else {
            return false;
        };
        if h.len == 0 || h.offset < pos || end > size {
            return false;
        }
        pos = end;
    }
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub blobs: Vec<BlobId>,
    /// Sorted, not overlapping, not empty.
    pub holes: Vec<Hole>,
}

/// What lies at a file offset: a hole up to an offset, or data (at an offset of the data
/// stream) up to an offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Hole { end: u64 },
    Data { data_offset: u64, end: u64 },
}

impl Content {
    pub fn hole_bytes(&self) -> u64 {
        self.holes.iter().map(|h| h.len).sum()
    }

    /// What's at file offset `pos` of a file `size` bytes long (`pos < size`).
    pub fn span_at(&self, pos: u64, size: u64) -> Span {
        let i = self.holes.partition_point(|h| h.offset + h.len <= pos);
        let before: u64 = self.holes[..i].iter().map(|h| h.len).sum();
        match self.holes.get(i) {
            Some(h) if h.offset <= pos => Span::Hole {
                end: (h.offset + h.len).min(size),
            },
            next => Span::Data {
                data_offset: pos - before,
                end: next.map_or(size, |h| h.offset),
            },
        }
    }

    /// The file ranges that hold data, in order: where the data stream goes.
    pub fn data_runs(&self, size: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::with_capacity(self.holes.len() + 1);
        let mut pos = 0;
        for h in &self.holes {
            if h.offset > pos {
                out.push((pos, h.offset - pos));
            }
            pos = h.offset + h.len;
        }
        if size > pos {
            out.push((pos, size - pos));
        }
        out
    }

    /// The holes of a file `size` bytes long whose data lies in `runs` (sorted).
    pub fn holes_between(runs: &[(u64, u64)], size: u64) -> Vec<Hole> {
        let mut out = Vec::new();
        let mut pos = 0;
        for &(start, len) in runs {
            if start > pos {
                out.push(Hole {
                    offset: pos,
                    len: start - pos,
                });
            }
            pos = start + len;
        }
        if size > pos {
            out.push(Hole {
                offset: pos,
                len: size - pos,
            });
        }
        out
    }
}

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
    /// Files: the holes left out of `content` (format 2; always empty in format 1).
    pub holes: Vec<Hole>,
}

/// A node as format 1 stores it: [`Node`] without `holes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NodeV1 {
    #[serde(with = "serde_bytes")]
    name: Vec<u8>,
    kind: Kind,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: i64,
    mtime_nsec: u32,
    dev: u64,
    ino: u64,
    nlink: u64,
    rdev: u64,
    #[serde(with = "serde_bytes")]
    link: Option<Vec<u8>>,
    hash: Option<[u8; 32]>,
    xattrs: Vec<Xattr>,
    content: Vec<BlobId>,
    subtree: Option<BlobId>,
}

impl From<NodeV1> for Node {
    fn from(n: NodeV1) -> Self {
        Node {
            name: n.name,
            kind: n.kind,
            mode: n.mode,
            uid: n.uid,
            gid: n.gid,
            size: n.size,
            mtime: n.mtime,
            mtime_nsec: n.mtime_nsec,
            dev: n.dev,
            ino: n.ino,
            nlink: n.nlink,
            rdev: n.rdev,
            link: n.link,
            hash: n.hash,
            xattrs: n.xattrs,
            content: n.content,
            subtree: n.subtree,
            holes: Vec::new(),
        }
    }
}

impl Node {
    /// The file's content (blobs and holes).
    pub fn file_content(&self) -> Content {
        Content {
            blobs: self.content.clone(),
            holes: self.holes.clone(),
        }
    }

    fn into_v1(self) -> Option<NodeV1> {
        self.holes.is_empty().then_some(NodeV1 {
            name: self.name,
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
            link: self.link,
            hash: self.hash,
            xattrs: self.xattrs,
            content: self.content,
            subtree: self.subtree,
        })
    }

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
            holes: Vec::new(),
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

#[derive(Serialize, Deserialize)]
struct TreeV1 {
    nodes: Vec<NodeV1>,
}

impl Tree {
    /// A tree blob's plaintext, in the encoding of tree format `format`.
    pub fn decode(raw: &[u8], format: u32) -> Result<Tree, postcard::Error> {
        if format >= 2 {
            let t: Tree = postcard::from_bytes(raw)?;
            // Holes in order, not empty, inside the file: anything else is a damaged or
            // forged tree, and the arithmetic of restoring it would go wrong. (Found by
            // fuzzing: fuzz/fuzz_targets/vault_tree.rs.)
            if !t.nodes.iter().all(|n| holes_are_sound(&n.holes, n.size)) {
                return Err(postcard::Error::SerdeDeCustom);
            }
            Ok(t)
        } else {
            let t: TreeV1 = postcard::from_bytes(raw)?;
            Ok(Tree {
                nodes: t.nodes.into_iter().map(Node::from).collect(),
            })
        }
    }

    /// Encode for tree format `format`. Format 1 has no holes: a node with holes can't be
    /// written in it (the backup reads files densely for format 1 repositories).
    pub fn encode(self, format: u32) -> Result<Vec<u8>, postcard::Error> {
        // What `decode` would refuse is never written.
        if self
            .nodes
            .iter()
            .any(|n| !holes_are_sound(&n.holes, n.size))
        {
            return Err(postcard::Error::SerializeBufferFull);
        }
        if format >= 2 {
            postcard::to_allocvec(&self)
        } else {
            let nodes = self
                .nodes
                .into_iter()
                .map(|n| n.into_v1().ok_or(postcard::Error::SerializeBufferFull))
                .collect::<Result<Vec<_>, _>>()?;
            postcard::to_allocvec(&TreeV1 { nodes })
        }
    }
}

/// `snapshots/<id>` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub meta: SnapshotMeta,
    /// Root tree id (hex).
    pub tree: String,
    pub parent: Option<String>,
    /// How this snapshot's trees are encoded: 1 (no holes) or 2. Missing in records
    /// written before format 2, which are all 1.
    #[serde(default = "tree_format_1", skip_serializing_if = "is_tree_format_1")]
    pub tree_format: u32,
}

fn tree_format_1() -> u32 {
    1
}

fn is_tree_format_1(f: &u32) -> bool {
    *f == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inputs that made fuzzing crash, kept (tests/data/fuzz-regressions/vault_tree):
    /// decoding them must fail cleanly, and nothing done with a tree that decodes may panic.
    #[test]
    fn fuzz_regressions_for_trees() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/fuzz-regressions/vault_tree");
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            n += 1;
            let raw = std::fs::read(e.path()).unwrap();
            for format in [1, 2] {
                if let Ok(t) = Tree::decode(&raw, format) {
                    for node in &t.nodes {
                        let c = node.file_content();
                        let _ = c.data_runs(node.size);
                        let _ = c.span_at(node.size / 2, node.size);
                    }
                }
            }
        }
        assert!(n > 0);
        // The first crash: holes out of order.
        let bad = Tree {
            nodes: vec![Node {
                holes: vec![Hole { offset: 10, len: 5 }, Hole { offset: 2, len: 1 }],
                size: 100,
                ..sample_node()
            }],
        };
        let raw = postcard::to_allocvec(&bad).unwrap();
        assert!(Tree::decode(&raw, 2).is_err());
        assert!(holes_are_sound(
            &[Hole { offset: 0, len: 5 }, Hole { offset: 5, len: 5 }],
            10
        ));
        assert!(!holes_are_sound(&[Hole { offset: 0, len: 0 }], 10));
        assert!(!holes_are_sound(
            &[Hole {
                offset: u64::MAX,
                len: 2
            }],
            u64::MAX
        ));
        assert!(!holes_are_sound(&[Hole { offset: 5, len: 6 }], 10));
    }

    fn sample_node() -> Node {
        Node {
            name: b"f".to_vec(),
            kind: flux_fs::entry::Kind::File,
            mode: 0o644,
            uid: 0,
            gid: 0,
            size: 0,
            mtime: 0,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            nlink: 1,
            rdev: 0,
            link: None,
            hash: None,
            xattrs: Vec::new(),
            content: Vec::new(),
            subtree: None,
            holes: Vec::new(),
        }
    }

    fn content(holes: &[(u64, u64)]) -> Content {
        Content {
            blobs: Vec::new(),
            holes: holes
                .iter()
                .map(|&(offset, len)| Hole { offset, len })
                .collect(),
        }
    }

    #[test]
    fn spans_and_runs() {
        // 100 bytes: data 0..10, hole 10..40, data 40..50, hole 50..100.
        let c = content(&[(10, 30), (50, 50)]);
        assert_eq!(c.hole_bytes(), 80);
        assert_eq!(c.data_runs(100), [(0, 10), (40, 10)]);
        assert_eq!(
            c.span_at(0, 100),
            Span::Data {
                data_offset: 0,
                end: 10
            }
        );
        assert_eq!(
            c.span_at(9, 100),
            Span::Data {
                data_offset: 9,
                end: 10
            }
        );
        assert_eq!(c.span_at(10, 100), Span::Hole { end: 40 });
        assert_eq!(c.span_at(39, 100), Span::Hole { end: 40 });
        assert_eq!(
            c.span_at(45, 100),
            Span::Data {
                data_offset: 15,
                end: 50
            }
        );
        assert_eq!(c.span_at(99, 100), Span::Hole { end: 100 });
        assert_eq!(Content::holes_between(&c.data_runs(100), 100), c.holes);
        // Starts with a hole; no holes at all.
        let c = content(&[(0, 8)]);
        assert_eq!(c.data_runs(20), [(8, 12)]);
        assert_eq!(
            c.span_at(8, 20),
            Span::Data {
                data_offset: 0,
                end: 20
            }
        );
        assert_eq!(content(&[]).data_runs(5), [(0, 5)]);
        assert_eq!(Content::holes_between(&[(0, 5)], 5), []);
        assert_eq!(Content::holes_between(&[], 7), [Hole { offset: 0, len: 7 }]);
    }

    fn node(holes: Vec<Hole>) -> Node {
        let e = Entry {
            path: b"f".to_vec(),
            kind: Kind::File,
            mode: 0o644,
            uid: 1,
            gid: 1,
            size: 64,
            mtime: 5,
            mtime_nsec: 0,
            dev: 0,
            ino: 9,
            nlink: 1,
            rdev: 0,
            link: None,
            hash: None,
            xattrs: Vec::new(),
        };
        let mut n = Node::from_entry(&e, b"f".to_vec());
        n.content = vec![[7u8; 32]];
        n.holes = holes;
        n
    }

    #[test]
    fn trees_in_both_formats() {
        let dense = Tree {
            nodes: vec![node(Vec::new())],
        };
        let v1 = dense.clone().encode(1).unwrap();
        let v2 = dense.clone().encode(2).unwrap();
        assert_ne!(v1, v2, "the formats differ, so their tree ids do too");
        assert_eq!(Tree::decode(&v1, 1).unwrap().nodes, dense.nodes);
        assert_eq!(Tree::decode(&v2, 2).unwrap().nodes, dense.nodes);

        let sparse = Tree {
            nodes: vec![node(vec![Hole { offset: 0, len: 32 }])],
        };
        assert!(sparse.clone().encode(1).is_err(), "format 1 has no holes");
        let raw = sparse.clone().encode(2).unwrap();
        assert_eq!(Tree::decode(&raw, 2).unwrap().nodes, sparse.nodes);
    }

    #[test]
    fn old_records_are_tree_format_1() {
        let json = r#"{"tree":"00","parent":null,"meta":null}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(v.get("tree_format").is_none());
        assert_eq!(tree_format_1(), 1);
        assert!(is_tree_format_1(&1) && !is_tree_format_1(&2));
    }
}
