//! Verify a Vault: structure always, every stored byte on request.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

use rayon::prelude::*;
use serde::Serialize;

use flux_core::progress::{Event, Phase, Progress, Reporter};

use crate::VaultError;
use crate::backend::pack_path;
use crate::crypto::{BlobId, hex_id, parse_id};
use crate::pack::{decode_blob, read_header, split_pack};
use crate::repo::VaultRepo;
use flux_core::sync::Locked;

#[derive(Debug, Default, Serialize)]
pub struct CheckReport {
    pub snapshots: u64,
    pub trees: u64,
    pub packs: u64,
    pub blobs: u64,
    /// Packs fully read and verified (`--read-data`).
    pub packs_read: u64,
    pub bytes_read: u64,
    /// Problems that make data unrestorable.
    pub errors: Vec<String>,
    /// Harmless findings (e.g. unreferenced packs that prune will remove).
    pub warnings: Vec<String>,
}

impl CheckReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn check(
    repo: &VaultRepo,
    read_data: bool,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<CheckReport, VaultError> {
    let _lock = repo.lock(false)?;
    let mut r = CheckReport::default();
    progress.set_phase(Phase::Scanning);
    report(Event::Phase {
        phase: Phase::Scanning,
    });

    // 1. Index vs. packs on disk.
    repo.invalidate_index();
    let index = repo.index()?;
    let on_disk: HashMap<String, u64> = repo
        .backend()
        .list("data")
        .map_err(|e| VaultError::io("data", e))?
        .into_iter()
        .collect();
    let mut by_pack: HashMap<BlobId, Vec<BlobId>> = HashMap::new();
    for (id, loc) in &index.map {
        by_pack.entry(loc.pack).or_default().push(*id);
    }
    r.packs = by_pack.len() as u64;
    r.blobs = index.map.len() as u64;
    let errors = Mutex::new(Vec::new());
    by_pack.par_iter().for_each(|(pack, blobs)| {
        let hex = hex_id(pack);
        let Some(&size) = on_disk.get(&hex) else {
            errors.locked().push(format!(
                "pack {hex} is missing ({} blobs lost)",
                blobs.len()
            ));
            return;
        };
        match read_header(repo.backend(), &repo.crypto, &hex, size) {
            Ok(header) => {
                let ids: HashSet<BlobId> = header.iter().map(|e| e.id).collect();
                if let Some(b) = blobs.iter().find(|b| !ids.contains(*b)) {
                    errors.locked().push(format!(
                        "pack {hex} lacks blob {} listed in the index",
                        hex_id(b)
                    ));
                }
            }
            Err(e) => errors.locked().push(format!("pack {hex}: {e}")),
        }
    });
    r.errors.extend(errors.into_inner().expect("errors"));
    let indexed: HashSet<String> = by_pack.keys().map(|p| hex_id(p)).collect();
    for name in on_disk.keys().filter(|n| !indexed.contains(*n)) {
        r.warnings.push(format!(
            "pack {name} is not referenced by the index (an interrupted backup; prune removes it)"
        ));
    }

    // 2. Every snapshot's trees and data blobs are present.
    let (records, unreadable) = repo.record_listing()?;
    for (name, e) in unreadable {
        r.errors.push(format!("snapshot {name} can't be read: {e}"));
    }
    r.snapshots = records.len() as u64;
    let mut seen_trees: HashSet<BlobId> = HashSet::new();
    let mut used_data: HashSet<BlobId> = HashSet::new();
    for rec in &records {
        let Some(root) = parse_id(&rec.tree) else {
            r.errors
                .push(format!("snapshot {}: bad root tree id", rec.meta.id));
            continue;
        };
        let mut stack = vec![root];
        while let Some(t) = stack.pop() {
            if !seen_trees.insert(t) {
                continue;
            }
            match repo.load_tree(&t, rec.tree_format) {
                Ok(tree) => {
                    for n in tree.nodes {
                        stack.extend(n.subtree);
                        if !holes_fit(&n.holes, n.size) {
                            r.errors.push(format!(
                                "snapshot {}: the holes recorded for /{} don't fit the file",
                                rec.meta.id,
                                String::from_utf8_lossy(&n.name),
                            ));
                        }
                        used_data.extend(n.content.iter().copied());
                        for b in &n.content {
                            if !index.contains(b) {
                                r.errors.push(format!(
                                    "snapshot {}: data of /{} is missing (blob {})",
                                    rec.meta.id,
                                    String::from_utf8_lossy(&n.name),
                                    hex_id(b)
                                ));
                            }
                        }
                    }
                }
                Err(e) => r.errors.push(format!("snapshot {}: {e}", rec.meta.id)),
            }
        }
    }
    r.trees = seen_trees.len() as u64;

    // Data no snapshot uses: left by deletions before space was freed, or by interrupted
    // backups. Harmless, but it takes space.
    let (unused_blobs, unused_bytes) = index
        .map
        .iter()
        .filter(|(id, _)| !seen_trees.contains(*id) && !used_data.contains(*id))
        .fold((0u64, 0u64), |(n, b), (_, loc)| {
            (n + 1, b + u64::from(loc.entry.length))
        });
    if unused_blobs > 0 {
        r.warnings.push(format!(
            "{} ({unused_blobs} blobs) belongs to no snapshot; pruning (\"Free up space\") removes it",
            flux_core::ByteSize(unused_bytes).display_approx()
        ));
    }

    // 3. Optionally read and verify every pack.
    if read_data {
        progress.set_phase(Phase::Transferring);
        report(Event::Phase {
            phase: Phase::Transferring,
        });
        let packs: Vec<(&String, &u64)> = on_disk
            .iter()
            .filter(|(n, _)| indexed.contains(*n))
            .collect();
        progress
            .bytes_total
            .store(packs.iter().map(|(_, s)| **s).sum(), Relaxed);
        progress.entries_total.store(packs.len() as u64, Relaxed);
        let errors = Mutex::new(Vec::new());
        packs.par_iter().for_each(|(name, size)| {
            if progress.is_cancelled() {
                return;
            }
            let path = pack_path(name);
            let result = (|| -> Result<(), String> {
                let bytes = repo.backend().read(&path).map_err(|e| e.to_string())?;
                if hex_id(blake3::hash(&bytes).as_bytes()) != **name {
                    return Err("contents do not match the pack id".into());
                }
                for e in split_pack(&repo.crypto, name, &bytes).map_err(|e| e.to_string())? {
                    let sealed = &bytes[e.offset as usize..e.offset as usize + e.length as usize];
                    decode_blob(&repo.crypto, sealed, &e).map_err(|e| e.to_string())?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                errors.locked().push(format!("pack {name}: {e}"));
            }
            progress.bytes_done.fetch_add(**size, Relaxed);
            progress.entries_done.fetch_add(1, Relaxed);
        });
        r.errors.extend(errors.into_inner().expect("errors"));
        r.packs_read = packs.len() as u64;
        r.bytes_read = packs.iter().map(|(_, s)| **s).sum();
        if progress.is_cancelled() {
            return Err(VaultError::Cancelled);
        }
    }
    r.errors.sort();
    progress.set_phase(Phase::Done);
    Ok(r)
}

/// Holes in order, not overlapping, not empty, and inside the file.
fn holes_fit(holes: &[crate::format::Hole], size: u64) -> bool {
    let mut pos = 0u64;
    holes.iter().all(|h| {
        let ok =
            h.len > 0 && h.offset >= pos && h.offset.checked_add(h.len).is_some_and(|e| e <= size);
        pos = h.offset.saturating_add(h.len);
        ok
    })
}

#[cfg(test)]
mod tests {
    use super::holes_fit;
    use crate::format::Hole;

    #[test]
    fn hole_lists_are_checked() {
        let h = |offset, len| Hole { offset, len };
        assert!(holes_fit(&[], 0));
        assert!(holes_fit(&[h(0, 4), h(8, 2)], 10));
        assert!(!holes_fit(&[h(8, 2), h(0, 4)], 10), "out of order");
        assert!(!holes_fit(&[h(0, 4), h(2, 4)], 10), "overlapping");
        assert!(!holes_fit(&[h(8, 4)], 10), "past the end");
        assert!(!holes_fit(&[h(3, 0)], 10), "empty");
        assert!(!holes_fit(&[h(u64::MAX, 2)], 10), "overflow");
    }
}
