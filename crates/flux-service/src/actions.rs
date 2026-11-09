//! Snapshot actions, comparisons, activity log, maintenance and restore with progress.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use serde::Serialize;

use flux_core::Engine;
use flux_core::progress::{Progress, ProgressSnapshot, silent};
use flux_fs::diff::{Change, DiffSummary, diff};
use flux_fs::entry::Kind;
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
    /// Comparing with the files on disk: how many files there are larger than the
    /// profile's size limit, which its backups leave out (so they're not compared).
    left_out_by_size: u64,
    /// That limit, as written in the profile (`None`: no limit, or one Borg ignores).
    size_limit: Option<String>,
}

#[cfg(test)]
impl Comparison {
    pub fn added(&self) -> u64 {
        self.summary.added
    }
    pub fn removed(&self) -> u64 {
        self.summary.removed
    }
    pub fn left_out_by_size(&self) -> u64 {
        self.left_out_by_size
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
    let mut left_out_by_size = 0;
    let mut size_limit = None;
    let (mut changes, summary, new_label) = match new {
        Some(n) => {
            let new_snap = repo.find(&n)?;
            let (c, s) = diff(repo.entries(&old_snap)?, repo.entries(&new_snap)?, b"")?;
            (c, s, new_snap.id)
        }
        None => {
            // Scan without the size limit, then drop (and count) what it leaves out: one
            // pass, and the app can say why big files aren't in the comparison. Borg
            // ignores the limit, so its backups hold those files and they stay in.
            let mut unlimited = p.clone();
            let limit = unlimited
                .sources
                .exclude_larger_than
                .take()
                .filter(|_| p.engine != Engine::Borg);
            let mut live =
                flux_runner::repo::scan_live(&pid, &unlimited, &Progress::new(), &silent)?;
            let old_entries: Vec<_> = repo
                .entries(&old_snap)?
                .collect::<std::result::Result<_, _>>()?;
            if let Some(max) = limit {
                // A big file the snapshot holds (backed up before the limit, or smaller
                // then) is still compared; the rest would never be backed up.
                let backed: HashSet<&[u8]> = old_entries
                    .iter()
                    .filter(|e| e.kind == Kind::File)
                    .map(|e| e.path.as_slice())
                    .collect();
                live.retain(|e| {
                    let left_out = e.kind == Kind::File
                        && e.size > max.bytes()
                        && !backed.contains(e.path.as_slice());
                    left_out_by_size += u64::from(left_out);
                    !left_out
                });
                size_limit = Some(max.to_string());
            }
            let (c, s) = diff(
                old_entries.into_iter().map(Ok::<_, std::io::Error>),
                live.into_iter().map(Ok),
                b"",
            )?;
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
        left_out_by_size,
        size_limit,
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
    exact: bool,
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
    let filter = exact
        .then(|| flux_fs::scan::exact_filter(&p))
        .transpose()
        .map_err(|e| ApiError::new("error", e.to_string()))?;
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
        exact: filter.as_ref(),
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
