//! Snapshot actions, comparisons, activity log, maintenance and restore with progress.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use serde::Serialize;

use flux_core::progress::{Progress, ProgressSnapshot, silent};
use flux_fs::diff::{Change, DiffSummary, diff};
use flux_fs::fsops::XattrPolicy;
use flux_fs::outcome::CheckSummary;
use flux_fs::restore::{RestoreOutcome, RestoreRequest};
use flux_fs::safe::LinkPolicy;
use flux_fs::snapshot::SnapshotMeta;
use flux_runner::repo::PruneSummary;
use flux_runner::runlog::{self, LoggedEvent, RunRecord};

use crate::{ApiError, Result, Service};

/// Changes shown at most in one comparison (the summary always covers everything).
const MAX_CHANGES: usize = 5000;

pub fn update_snapshot_impl(
    state: &Service,
    id: String,
    snapshot: String,
    name: Option<String>,
    set_name: bool,
    pinned: Option<bool>,
) -> Result<SnapshotMeta> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    if let Some(n) = name.as_deref()
        && repo
            .snapshots()?
            .iter()
            .any(|s| s.id != snapshot && s.name.as_deref() == Some(n))
    {
        return Err(ApiError::new(
            "error",
            format!("another snapshot is already named “{n}”"),
        ));
    }
    let name = set_name.then(|| name.filter(|n| !n.trim().is_empty()));
    Ok(repo.update(&snapshot, name, pinned)?)
}

/// Delete a snapshot. Returns the bytes freed (Vaults; `None` for Mirrors).
pub fn delete_snapshot_impl(state: &Service, id: String, snapshot: String) -> Result<Option<u64>> {
    let (pid, p) = state.load(&id)?;
    let freed = state
        .open(&pid, &p)?
        .delete(&[snapshot.as_str()], &silent)?;
    state.forget_cached(&pid);
    Ok(freed)
}

#[derive(Serialize)]
pub struct Comparison {
    old: String,
    new: String,
    summary: DiffSummary,
    changes: Vec<Change>,
    truncated: bool,
}

#[cfg(test)]
impl Comparison {
    pub fn added(&self) -> u64 {
        self.summary.added
    }
}

/// Compare two snapshots, or a snapshot with the files on disk now (`new = None`).
pub fn compare_impl(
    state: &Service,
    id: String,
    old: String,
    new: Option<String>,
) -> Result<Comparison> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let old_snap = repo.find(&old)?;
    let (mut changes, summary, new_label) = match new {
        Some(n) => {
            let new_snap = repo.find(&n)?;
            let (c, s) = diff(repo.entries(&old_snap)?, repo.entries(&new_snap)?, b"")?;
            (c, s, new_snap.id)
        }
        None => {
            let live = flux_runner::repo::scan_live(&pid, &p, &Progress::new(), &silent)?;
            let (c, s) = diff(repo.entries(&old_snap)?, live.into_iter().map(Ok), b"")?;
            (c, s, "now".to_owned())
        }
    };
    let truncated = changes.len() > MAX_CHANGES;
    changes.truncate(MAX_CHANGES);
    Ok(Comparison {
        old: old_snap.id,
        new: new_label,
        summary,
        changes,
        truncated,
    })
}

pub fn runs_impl(state: &Service, id: String, limit: usize) -> Result<Vec<RunRecord>> {
    let (pid, _) = state.load(&id)?;
    Ok(runlog::recent_runs(
        &state.paths,
        &pid,
        limit.clamp(1, 1000),
    ))
}

pub fn run_events_impl(state: &Service, id: String, run_id: String) -> Result<Vec<LoggedEvent>> {
    let (pid, _) = state.load(&id)?;
    Ok(runlog::run_events(&state.paths, &pid, &run_id))
}

pub fn prune_impl(state: &Service, id: String, dry_run: bool) -> Result<PruneSummary> {
    let (pid, p) = state.load(&id)?;
    let r = state
        .open(&pid, &p)?
        .prune(&p.retention, dry_run, &silent)?;
    state.forget_cached(&pid);
    Ok(r)
}

pub fn check_impl(state: &Service, id: String, read_data: bool) -> Result<CheckSummary> {
    let (pid, p) = state.load(&id)?;
    Ok(state
        .open(&pid, &p)?
        .check(read_data, &Progress::new(), &silent)?)
}

/// Restore, calling `on_progress` about five times a second while it runs.
#[allow(clippy::too_many_arguments)]
pub fn restore_impl(
    state: &Service,
    id: String,
    snapshot: String,
    paths: Vec<String>,
    to: Option<String>,
    overwrite: bool,
    on_progress: &(dyn Fn(ProgressSnapshot) + Sync),
) -> Result<RestoreOutcome> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snap = repo.find(&snapshot)?;
    let to = to.map(PathBuf::from);
    if let Some(t) = &to
        && !t.is_absolute()
    {
        return Err(ApiError::new(
            "error",
            "the destination must be an absolute path",
        ));
    }
    let req = RestoreRequest {
        snapshot: &snap,
        paths: paths.into_iter().map(PathBuf::from).collect(),
        to,
        keep_existing: p.options.backup_before_restore && !overwrite,
        dry_run: false,
        xattrs: XattrPolicy {
            acl: p.options.preserve_acl,
            other: p.options.preserve_xattr,
        },
        links: LinkPolicy::for_process(),
    };
    let progress = Progress::new();
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Relaxed) {
                on_progress(progress.snapshot());
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let r = repo.restore(&req, &progress, &silent);
        done.store(true, Relaxed);
        r
    });
    Ok(result?)
}
