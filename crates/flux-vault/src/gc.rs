//! Retention for Vault snapshots, and garbage collection of the data they no longer use.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use serde::Serialize;

use flux_core::profile::Retention;
use flux_core::progress::{Event, Reporter};
use flux_core::retention::{self, Decision, SnapshotInfo};
use flux_fs::snapshot::{SnapshotMeta, SnapshotStatus};

use crate::VaultError;
use crate::backend::pack_path;
use crate::crypto::{BlobId, hex_id, parse_id};
use crate::format::{BlobKind, IndexFile, IndexPack};
use crate::pack::{Packer, split_pack, write_index};
use crate::repo::{Owner, VaultRepo};

/// Repack a pack when more than this share of its bytes is unused.
const REPACK_THRESHOLD: f64 = 0.2;

#[derive(Debug, Default, Serialize)]
pub struct GcReport {
    pub packs_deleted: u64,
    pub packs_repacked: u64,
    /// Packs that no index referenced (left by interrupted backups).
    pub orphan_packs: u64,
    pub bytes_freed: u64,
}

#[derive(Debug, Serialize)]
pub struct PruneReport {
    pub decisions: Vec<Decision>,
    pub removed: Vec<String>,
    pub gc: Option<GcReport>,
    pub dry_run: bool,
}

pub fn info(m: &SnapshotMeta) -> SnapshotInfo {
    SnapshotInfo {
        id: m.id.clone(),
        time: m.started,
        named: m.name.is_some(),
        pinned: m.pinned,
        healthy: m.status == SnapshotStatus::Complete,
    }
}

/// Forget `owner`'s snapshots that the policy doesn't keep. Needs no exclusive lock: it only
/// removes snapshot records; space is reclaimed by [`gc`].
pub fn forget(
    repo: &VaultRepo,
    owner: &Owner,
    policy: &Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<(Vec<Decision>, Vec<String>), VaultError> {
    let snaps = repo.snapshots(owner)?;
    let infos: Vec<SnapshotInfo> = snaps.iter().map(info).collect();
    let decisions = retention::evaluate(
        &infos,
        policy,
        Timestamp::now(),
        &jiff::tz::TimeZone::system(),
    );
    let mut removed = Vec::new();
    for d in decisions.iter().rev().filter(|d| !d.keep) {
        if !dry_run {
            repo.delete_record(&d.id)?;
            report(Event::Info {
                message: format!("forgot snapshot {}", d.id),
            });
        }
        removed.push(d.id.clone());
    }
    Ok((decisions, removed))
}

/// Forget per policy, then garbage-collect. Takes the exclusive lock.
pub fn prune(
    repo: &VaultRepo,
    owner: &Owner,
    policy: &Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<PruneReport, VaultError> {
    let _lock = repo.lock(true)?;
    let (decisions, removed) = forget(repo, owner, policy, dry_run, report)?;
    let gc = if dry_run {
        None
    } else {
        Some(gc_locked(repo, report)?)
    };
    Ok(PruneReport {
        decisions,
        removed,
        gc,
        dry_run,
    })
}

/// Delete snapshots by id and free the data only they used, at once. Other snapshots in
/// the Vault (of any profile or computer) keep everything they need. Takes the exclusive
/// lock.
pub fn delete(
    repo: &VaultRepo,
    ids: &[&str],
    report: Reporter<'_>,
) -> Result<GcReport, VaultError> {
    let _lock = repo.lock(true)?;
    for id in ids {
        repo.delete_record(id)?;
    }
    gc_locked(repo, report)
}

/// Delete data no snapshot uses, and nothing else: no snapshot is forgotten. Takes the
/// exclusive lock.
pub fn collect(repo: &VaultRepo, report: Reporter<'_>) -> Result<GcReport, VaultError> {
    let _lock = repo.lock(true)?;
    gc_locked(repo, report)
}

/// Delete data no snapshot uses. Caller holds the exclusive lock.
pub fn gc_locked(repo: &VaultRepo, report: Reporter<'_>) -> Result<GcReport, VaultError> {
    let mut out = GcReport::default();
    repo.invalidate_index();
    let index = repo.index()?;

    // Everything still reachable from any snapshot (all owners).
    let mut used: HashSet<BlobId> = HashSet::new();
    // Every record, or stop: a snapshot left out here would lose its data below.
    for rec in repo.records_strict()? {
        let root = parse_id(&rec.tree)
            .ok_or_else(|| VaultError::Corrupt(format!("snapshot {}: bad tree id", rec.meta.id)))?;
        let mut stack = vec![root];
        while let Some(t) = stack.pop() {
            if !used.insert(t) {
                continue;
            }
            for n in repo.load_tree(&t, rec.tree_format)?.nodes {
                stack.extend(n.subtree);
                used.extend(n.content);
            }
        }
    }

    // Classify packs.
    let mut packs: HashMap<BlobId, Vec<crate::format::PackEntry>> = HashMap::new();
    for loc in index.map.values() {
        packs.entry(loc.pack).or_default().push(loc.entry);
    }
    let sizes: HashMap<String, u64> = repo
        .backend()
        .list("data")
        .map_err(|e| VaultError::io("data", e))?
        .into_iter()
        .collect();
    let mut keep: Vec<IndexPack> = Vec::new();
    let mut doomed: Vec<String> = Vec::new();
    let mut repack: Vec<(BlobId, Vec<crate::format::PackEntry>)> = Vec::new();
    for (pack, mut blobs) in packs {
        blobs.sort_by_key(|b| b.offset);
        let total: u64 = blobs.iter().map(|b| u64::from(b.length)).sum();
        let unused: u64 = blobs
            .iter()
            .filter(|b| !used.contains(&b.id))
            .map(|b| u64::from(b.length))
            .sum();
        if unused == total {
            doomed.push(hex_id(&pack));
        } else if total > 0 && unused as f64 / total as f64 > REPACK_THRESHOLD {
            repack.push((pack, blobs));
        } else {
            keep.push(IndexPack { pack, blobs });
        }
    }
    let indexed: HashSet<String> = keep
        .iter()
        .map(|p| hex_id(&p.pack))
        .chain(doomed.iter().cloned())
        .chain(repack.iter().map(|(p, _)| hex_id(p)))
        .collect();
    let orphans: Vec<String> = sizes
        .keys()
        .filter(|n| !indexed.contains(*n))
        .cloned()
        .collect();
    out.orphan_packs = orphans.len() as u64;

    // Copy still-used blobs out of packs that are mostly garbage (no re-encryption needed).
    // Written through the cache: the drive is being read at the same time (see
    // `Backend::write_pack_cached`).
    let data_packer = Packer::for_repo(repo).cached();
    let tree_packer = Packer::for_repo(repo).cached();
    for (pack, _) in &repack {
        let hex = hex_id(pack);
        let path = pack_path(&hex);
        let bytes = repo
            .backend()
            .read(&path)
            .map_err(|e| VaultError::io(&path, e))?;
        for e in split_pack(&repo.crypto, &hex, &bytes)? {
            if used.contains(&e.id) {
                let sealed = &bytes[e.offset as usize..(e.offset + e.length) as usize];
                let packer = if e.kind == BlobKind::Tree {
                    &tree_packer
                } else {
                    &data_packer
                };
                packer.add(e.kind, e.id, e.raw_length as usize, sealed)?;
            }
        }
        doomed.push(hex);
        out.packs_repacked += 1;
    }
    let new_bytes = data_packer
        .bytes_written
        .load(std::sync::atomic::Ordering::Relaxed)
        + tree_packer
            .bytes_written
            .load(std::sync::atomic::Ordering::Relaxed);
    keep.extend(data_packer.finish()?);
    keep.extend(tree_packer.finish()?);

    // New index first, then remove old index files, then packs. A crash at any point
    // leaves every remaining snapshot readable.
    //
    // The new index supersedes every index file there is, the leftovers of an earlier
    // prune that was cut short included (`index.stale`): they are only ignored while the
    // file that names them exists, and that file is removed here.
    let old_index: Vec<String> = index.files.iter().chain(&index.stale).cloned().collect();
    if !doomed.is_empty() || !orphans.is_empty() || old_index.len() > 1 {
        // The new packs on disk before the index that names them.
        repo.backend()
            .flush_packs()
            .map_err(|e| VaultError::io("data", e))?;
        write_index(
            repo.backend(),
            &repo.crypto,
            &IndexFile {
                packs: keep,
                supersedes: old_index.clone(),
            },
            repo.parity,
        )?;
        for f in &old_index {
            let p = format!("index/{f}");
            repo.backend()
                .remove(&p)
                .map_err(|e| VaultError::io(&p, e))?;
            crate::protect::remove_for(repo.backend(), &p);
        }
        for name in doomed.iter().chain(&orphans) {
            let p = pack_path(name);
            repo.backend()
                .remove(&p)
                .map_err(|e| VaultError::io(&p, e))?;
            crate::protect::remove_for(repo.backend(), &p);
            out.bytes_freed += sizes.get(name).copied().unwrap_or(0);
        }
        out.bytes_freed = out.bytes_freed.saturating_sub(new_bytes);
        out.packs_deleted = (doomed.len() + orphans.len()) as u64 - out.packs_repacked;
        repo.invalidate_index();
        report(Event::Info {
            message: format!(
                "freed {} ({} packs deleted, {} repacked)",
                flux_core::ByteSize(out.bytes_freed).display_approx(),
                out.packs_deleted,
                out.packs_repacked
            ),
        });
    }
    // Parity files whose file is gone (deleted by a Flux that keeps none), and repair
    // data for the small files that lack it.
    crate::protect::tidy(repo)?;
    Ok(out)
}
