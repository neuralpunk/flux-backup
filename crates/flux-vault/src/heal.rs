//! Putting a Vault right where its repair data can't ([`crate::protect`] rebuilds damaged
//! files; this is for what is past that, or has no repair data at all).
//!
//! A backup stores a blob only if the index doesn't have it, and reuses an unchanged
//! file's blobs without reading the file. So a blob the index names but that can't be
//! read any more (its pack is damaged beyond repair, or gone) is never stored again,
//! however often the files that hold it are backed up: every new snapshot counts on it
//! too. [`drop_lost`] takes such blobs out of the index. The next backup then reads the
//! files that had them and stores them again, which also puts older snapshots right
//! where the files haven't changed since (the blob has the same id). What can't be had
//! back that way is what a check then reports, file by file.
//!
//! [`rebuild_index`] is for an index file that doesn't open: every pack ends with the list
//! of its own blobs, and the index is those lists.

use std::collections::{HashMap, HashSet};

use crate::VaultError;
use crate::backend::pack_path;
use crate::crypto::{BlobId, hex_id, parse_id};
use crate::format::{BlobKind, IndexFile, IndexPack, PackEntry};
use crate::pack::{Packer, decode_blob, read_headers, write_index};
use crate::protect;
use crate::repo::VaultRepo;

/// What became of one pack given to [`drop_lost`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub pack: String,
    /// Its blobs that still read and were moved to a new pack.
    pub saved: u64,
    /// Its blobs that are gone: no longer in the index.
    pub lost: u64,
}

/// Take the packs `lost` (ids in hex: damaged beyond repair, or missing) out of the
/// Vault. Each blob of theirs that still decodes and matches its id is moved to a new
/// pack; the others leave the index. Then the damaged packs are removed, with their
/// parity files. The caller holds the exclusive lock.
///
/// In the order a prune works: new packs, the new index (superseding every other), the
/// old index files, the old packs.
pub fn drop_lost(repo: &VaultRepo, lost: &HashSet<String>) -> Result<Vec<Dropped>, VaultError> {
    if lost.is_empty() {
        return Ok(Vec::new());
    }
    let backend = repo.backend();
    repo.invalidate_index();
    let index = repo.index()?;
    let sizes: HashMap<String, u64> = backend
        .list("data")
        .map_err(|e| VaultError::io("data", e))?
        .into_iter()
        .collect();
    let mut keep: HashMap<BlobId, Vec<PackEntry>> = HashMap::new();
    let mut theirs: HashMap<String, Vec<PackEntry>> = HashMap::new();
    for loc in index.map.values() {
        let hex = hex_id(&loc.pack);
        if lost.contains(&hex) {
            theirs.entry(hex).or_default().push(loc.entry);
        } else {
            keep.entry(loc.pack).or_default().push(loc.entry);
        }
    }
    let data_packer = Packer::for_repo(repo).cached();
    let tree_packer = Packer::for_repo(repo).cached();
    let mut out = Vec::new();
    for (hex, mut entries) in theirs {
        entries.sort_by_key(|e| e.offset);
        let path = pack_path(&hex);
        // What can be read of it (unreadable parts as zeros); nothing, if it is gone.
        let bytes = match sizes.get(&hex) {
            Some(&size) => protect::read_lossy(backend, &path, size)
                .map_err(|e| VaultError::io(&path, e))?
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut d = Dropped {
            pack: hex,
            saved: 0,
            lost: 0,
        };
        for e in entries {
            let sealed = (e.offset as usize)
                .checked_add(e.length as usize)
                .and_then(|end| bytes.get(e.offset as usize..end));
            match sealed {
                Some(sealed) if decode_blob(&repo.crypto, sealed, &e).is_ok() => {
                    let packer = if e.kind == BlobKind::Tree {
                        &tree_packer
                    } else {
                        &data_packer
                    };
                    packer.add(e.kind, e.id, e.raw_length as usize, sealed)?;
                    d.saved += 1;
                }
                _ => d.lost += 1,
            }
        }
        out.push(d);
    }
    let mut packs: Vec<IndexPack> = keep
        .into_iter()
        .map(|(pack, mut blobs)| {
            blobs.sort_by_key(|b| b.offset);
            IndexPack { pack, blobs }
        })
        .collect();
    packs.extend(data_packer.finish()?);
    packs.extend(tree_packer.finish()?);
    backend
        .flush_packs()
        .map_err(|e| VaultError::io("data", e))?;
    let old: Vec<String> = index.files.iter().chain(&index.stale).cloned().collect();
    replace_index(repo, packs, &old)?;
    for hex in lost {
        let p = pack_path(hex);
        backend.remove(&p).map_err(|e| VaultError::io(&p, e))?;
        protect::remove_for(backend, &p);
    }
    out.sort_by(|a, b| a.pack.cmp(&b.pack));
    Ok(out)
}

/// Write an index of `packs` that supersedes the index files `old`, then remove those.
fn replace_index(
    repo: &VaultRepo,
    packs: Vec<IndexPack>,
    old: &[String],
) -> Result<(), VaultError> {
    let backend = repo.backend();
    write_index(
        backend,
        &repo.crypto,
        &IndexFile {
            packs,
            supersedes: old.to_vec(),
        },
        repo.parity,
    )?;
    for f in old {
        let p = format!("index/{f}");
        backend.remove(&p).map_err(|e| VaultError::io(&p, e))?;
        protect::remove_for(backend, &p);
    }
    repo.invalidate_index();
    Ok(())
}

/// What [`rebuild_index`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuilt {
    /// Packs whose own list of blobs went into the new index.
    pub packs: u64,
    /// Packs whose list couldn't be read: left out (their blobs are then missing, and a
    /// check says from which snapshots).
    pub unreadable: Vec<String>,
}

/// Write the index again from the packs themselves (each ends with the list of its
/// blobs), replacing every index file there is, readable or not. For an index file that
/// doesn't open and has no repair data that rebuilds it. The caller holds the exclusive
/// lock.
///
/// The new index names every blob in every pack, also blobs no snapshot uses (what a
/// prune had dropped from the index but not yet from a pack it kept, or an interrupted
/// backup's packs): they are all there to be read, and the next prune sorts them out.
pub fn rebuild_index(repo: &VaultRepo) -> Result<Rebuilt, VaultError> {
    let backend = repo.backend();
    let on_disk = backend
        .list("data")
        .map_err(|e| VaultError::io("data", e))?;
    let mut packs = Vec::new();
    let mut unreadable = Vec::new();
    for group in on_disk.chunks(256) {
        let headers = read_headers(backend, &repo.crypto, group);
        for ((hex, size), header) in group.iter().zip(headers) {
            let blobs = header.ok().filter(|h| {
                h.iter()
                    .all(|e| u64::from(e.offset) + u64::from(e.length) <= *size)
            });
            match (parse_id(hex), blobs) {
                (Some(pack), Some(blobs)) => packs.push(IndexPack { pack, blobs }),
                _ => unreadable.push(hex.clone()),
            }
        }
    }
    if packs.is_empty() && !on_disk.is_empty() {
        return Err(VaultError::Corrupt(
            "the index can't be read, and neither can any pack's own list of what it holds".into(),
        ));
    }
    let old: Vec<String> = backend
        .list("index")
        .map_err(|e| VaultError::io("index", e))?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let n = packs.len() as u64;
    replace_index(repo, packs, &old)?;
    Ok(Rebuilt {
        packs: n,
        unreadable,
    })
}
