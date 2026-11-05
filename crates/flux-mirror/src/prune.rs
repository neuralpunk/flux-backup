//! Apply a retention policy to a Mirror repository.

use std::collections::HashSet;
use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;

use flux_core::profile::Retention;
use flux_core::progress::{Event, Reporter};
use flux_core::retention::{self, Decision, SnapshotInfo};

use crate::MirrorError;
use crate::repo::{MirrorRepo, SnapshotMeta, SnapshotStatus};

#[derive(Debug, Serialize)]
pub struct PruneReport {
    /// Policy decision for every snapshot, newest first.
    pub decisions: Vec<Decision>,
    /// Snapshots removed (or that would be removed in a dry run) by the policy.
    pub removed: Vec<String>,
    /// Snapshots removed additionally to satisfy free-space / free-inode floors.
    pub removed_for_space: Vec<String>,
    /// The space floor still isn't met (nothing left that may be removed).
    pub space_floor_unmet: bool,
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

/// Free bytes and free-inode percentage of the filesystem holding `path`.
pub fn free_space(path: &Path) -> Option<(u64, f64)> {
    let v = rustix::fs::statvfs(path).ok()?;
    let bytes = v.f_bavail.saturating_mul(v.f_frsize);
    let inodes = if v.f_files == 0 {
        100.0
    } else {
        v.f_favail as f64 * 100.0 / v.f_files as f64
    };
    Some((bytes, inodes))
}

fn floors_met(path: &Path, policy: &Retention, extra_bytes: u64) -> bool {
    let Some((bytes, inodes)) = free_space(path) else {
        return true;
    };
    let space_ok = policy
        .min_free_space
        .is_none_or(|m| bytes >= m.bytes().saturating_add(extra_bytes));
    let inodes_ok = policy
        .min_free_inodes_percent
        .is_none_or(|m| inodes >= f64::from(m));
    space_ok && inodes_ok && bytes >= extra_bytes
}

/// Prune `repo` per `policy`. The caller must hold the repository lock.
pub fn prune_locked(
    repo: &MirrorRepo,
    policy: &Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<PruneReport, MirrorError> {
    let snaps = repo.snapshots()?;
    let infos: Vec<SnapshotInfo> = snaps.iter().map(info).collect();
    let decisions = retention::evaluate(
        &infos,
        policy,
        Timestamp::now(),
        &jiff::tz::TimeZone::system(),
    );
    let mut removed = Vec::new();
    // Oldest first, so an interrupted prune has removed the least valuable snapshots.
    for d in decisions.iter().rev().filter(|d| !d.keep) {
        if !dry_run {
            repo.delete_snapshot(&d.id)?;
            report(Event::Info {
                message: format!("pruned snapshot {}", d.id),
            });
        }
        removed.push(d.id.clone());
    }

    let mut removed_for_space = Vec::new();
    let mut space_floor_unmet = false;
    if policy.enabled
        && (policy.min_free_space.is_some() || policy.min_free_inodes_percent.is_some())
    {
        if dry_run {
            space_floor_unmet = !floors_met(repo.root(), policy, 0);
        } else {
            let gone: HashSet<String> = removed.iter().cloned().collect();
            let candidates = retention::space_candidates(&infos, policy, &gone);
            let mut it = candidates.into_iter();
            while !floors_met(repo.root(), policy, 0) {
                let Some(id) = it.next() else {
                    space_floor_unmet = true;
                    break;
                };
                repo.delete_snapshot(&id)?;
                report(Event::Info {
                    message: format!("removed snapshot {id} to free space"),
                });
                removed_for_space.push(id);
            }
        }
    }
    Ok(PruneReport {
        decisions,
        removed,
        removed_for_space,
        space_floor_unmet,
        dry_run,
    })
}

/// Remove the oldest prunable snapshots until `needed` bytes (plus the policy's free-space
/// floor) are available. Returns the removed ids. Never removes the newest, pinned or
/// protected-named snapshots. Caller holds the lock.
pub fn make_room(
    repo: &MirrorRepo,
    policy: &Retention,
    needed: u64,
    report: Reporter<'_>,
) -> Result<Vec<String>, MirrorError> {
    let mut removed = Vec::new();
    if !policy.enabled {
        return Ok(removed);
    }
    let infos: Vec<SnapshotInfo> = repo.snapshots()?.iter().map(info).collect();
    for id in retention::space_candidates(&infos, policy, &HashSet::new()) {
        if floors_met(repo.root(), policy, needed) {
            break;
        }
        repo.delete_snapshot(&id)?;
        report(Event::Info {
            message: format!("removed old snapshot {id} to make room"),
        });
        removed.push(id);
    }
    Ok(removed)
}
