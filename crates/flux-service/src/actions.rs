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

/// Change a snapshot's name, pin, note or tags. A name must be unique; an empty name
/// removes it.
pub fn update_snapshot_impl(
    state: &Service,
    id: String,
    snapshot: String,
    mut change: flux_fs::snapshot::SnapshotChange,
) -> Result<SnapshotMeta> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    if let Some(name) = &mut change.name {
        *name = name.take().filter(|n| !n.trim().is_empty());
    }
    if let Some(Some(n)) = change.name.as_ref().map(|n| n.as_deref())
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
    let r = repo.change(&snapshot, &change)?;
    state.forget_cached(&pid);
    Ok(r)
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

/// Test a restore now (the profile's settings: by default 10 random files).
pub fn restore_test_impl(
    state: &Service,
    id: String,
) -> Result<flux_runner::restore_test::TestReport> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    Ok(flux_runner::restore_test::run(
        &state.paths,
        &pid,
        &p,
        &p.restore_test,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    ))
}

/// The latest restore tests, newest first.
pub fn restore_tests_impl(
    state: &Service,
    id: String,
) -> Result<Vec<flux_runner::restore_test::TestReport>> {
    let (pid, _) = state.load(&id)?;
    let mut h = flux_runner::restore_test::load_state(&state.paths, &pid).history;
    h.reverse();
    Ok(h)
}

/// "This was me": the unusual changes of `snapshot` were the person's own. The warning
/// goes; the snapshot before them is unpinned (if Flux pinned it) and the tag removed.
pub fn dismiss_unusual_impl(state: &Service, id: String, snapshot: String) -> Result<()> {
    let (pid, p) = state.load(&id)?;
    flux_runner::dismiss_unusual(&state.paths, &pid, &snapshot)?;
    let run = flux_runner::runlog::recent_runs(&state.paths, &pid, 300)
        .into_iter()
        .find(|r| r.unusual.as_ref().is_some_and(|u| u.snapshot == snapshot));
    if let Ok(repo) = state.open(&pid, &p)
        && let Ok(snaps) = repo.snapshots()
    {
        {
            if let Some(s) = snaps.iter().find(|s| s.id == snapshot) {
                let tags: Vec<String> = s
                    .tags
                    .iter()
                    .filter(|t| *t != flux_runner::UNUSUAL_TAG)
                    .cloned()
                    .collect();
                let _ = repo.change(
                    &s.id,
                    &flux_fs::snapshot::SnapshotChange {
                        tags: Some(tags),
                        ..Default::default()
                    },
                );
            }
            if let Some(before) = run.and_then(|r| r.unusual).and_then(|u| u.before)
                && let Some(s) = snaps.iter().find(|s| s.id == before)
                && s.note
                    .as_deref()
                    .is_some_and(|n| n.starts_with(flux_runner::AUTO_PIN_NOTE))
            {
                let _ = repo.change(
                    &s.id,
                    &flux_fs::snapshot::SnapshotChange {
                        pinned: Some(false),
                        note: Some(None),
                        ..Default::default()
                    },
                );
            }
        }
    }
    state.forget_cached(&pid);
    Ok(())
}

/// Save files of a snapshot as an archive in folder `dir` (named after the backup, the
/// snapshot's date and, for one item, its name). `paths` empty: the whole snapshot.
#[allow(clippy::too_many_arguments)]
pub fn export_snapshot_impl(
    state: &Service,
    id: String,
    snapshot: String,
    paths: Vec<String>,
    format: flux_runner::archive::Format,
    dir: String,
    checksums: Option<flux_runner::archive::Checksums>,
    on_progress: &(dyn Fn(ProgressSnapshot) + Sync),
) -> Result<flux_runner::archive::ExportOutcome> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snap = repo.find(&snapshot)?;
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(ApiError::new(
            "error",
            "Choose a folder to save the archive in.",
        ));
    }
    let when = snap
        .started
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d-%H%M")
        .to_string();
    let what = match paths.as_slice() {
        [one] => std::path::Path::new(one)
            .file_name()
            .map(|n| format!("-{}", n.to_string_lossy().replace(['/', '\0'], "_")))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let base = format!("{}-{when}{what}", pid.as_str().replace('+', "-"));
    let mut out = dir.join(format!("{base}.{}", format.extension()));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{base}-{n}.{}", format.extension()));
        n += 1;
    }
    let progress = Progress::new();
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Relaxed) {
                on_progress(progress.snapshot());
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let r = flux_runner::archive::export(
            &repo,
            &flux_runner::archive::ExportRequest {
                snapshot: &snap,
                paths: paths.into_iter().map(PathBuf::from).collect(),
                format,
                out,
                checksums,
            },
            &progress,
            &silent,
        );
        done.store(true, Relaxed);
        r
    });
    result.map_err(|e| ApiError::new("error", e))
}

#[cfg(test)]
mod unusual_tests {
    use super::*;
    use flux_core::profile::Target;
    use flux_core::{Paths, ProfileId, ProfileStore, Scope};

    #[test]
    fn this_was_me_unpins_and_untags() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let docs = t.join("src/docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(t.join("tgt")).unwrap();
        for i in 0..40 {
            std::fs::write(docs.join(format!("f{i}.odt")), format!("doc {i}")).unwrap();
        }
        let paths = Paths::under(t.join("home"), Scope::User);
        let id = ProfileId::new("p").unwrap();
        let mut p = flux_core::Profile::new(
            "P",
            Target::Local {
                path: t.join("tgt"),
                fs_uuid: None,
            },
            vec![t.join("src")],
        );
        p.options.notify = false;
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let backup = || {
            flux_runner::run_backup(
                &flux_runner::BackupJob {
                    paths: &paths,
                    id: &id,
                    profile: &p,
                    name: None,
                    dry_run: false,
                    trigger: flux_runner::Trigger::Manual,
                    prune: true,
                    only_if_due: false,
                    ask: None,
                    shutdown_after: false,
                },
                &flux_core::progress::Progress::new(),
                &flux_core::progress::silent,
            )
        };
        backup();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        for i in 0..25 {
            let f = docs.join(format!("f{i}.odt"));
            std::fs::rename(&f, f.with_extension("odt.locked")).unwrap();
        }
        let u = backup().record.unusual.expect("flagged");
        let state = Service::new(paths.clone());
        let o = crate::views::overview_impl(&state, "p".into()).unwrap();
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["unusual"]["snapshot"], u.snapshot.as_str());
        assert_eq!(v["health"][0]["level"], "danger");

        dismiss_unusual_impl(&state, "p".into(), u.snapshot.clone()).unwrap();
        let snaps = crate::views::snapshots_impl(&state, "p".into()).unwrap();
        assert!(
            snaps.iter().all(|s| !s.pinned && s.note.is_none()),
            "{snaps:?}"
        );
        assert!(snaps.iter().all(|s| s.tags.is_empty()));
        let v =
            serde_json::to_value(crate::views::overview_impl(&state, "p".into()).unwrap()).unwrap();
        assert!(v["unusual"].is_null());
    }
}
#[cfg(test)]
mod export_tests {
    use super::*;
    use flux_core::profile::Target;
    use flux_core::{Paths, ProfileId, ProfileStore, Scope};
    use flux_runner::archive::{Checksums, Format};

    #[test]
    fn archives_are_named_and_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        std::fs::create_dir_all(t.join("src/docs")).unwrap();
        std::fs::create_dir_all(t.join("tgt")).unwrap();
        std::fs::create_dir_all(t.join("out")).unwrap();
        std::fs::write(t.join("src/docs/a"), "a").unwrap();
        let paths = Paths::under(t.join("home"), Scope::User);
        let id = ProfileId::new("p").unwrap();
        let mut p = flux_core::Profile::new(
            "P",
            Target::Local {
                path: t.join("tgt"),
                fs_uuid: None,
            },
            vec![t.join("src")],
        );
        p.options.notify = false;
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths: &paths,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: flux_runner::Trigger::Manual,
                prune: true,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &flux_core::progress::Progress::new(),
            &flux_core::progress::silent,
        );
        let state = Service::new(paths);
        let out = t.join("out").display().to_string();
        let docs = t.join("src/docs").display().to_string();
        let export = |paths: Vec<String>, sums| {
            export_snapshot_impl(
                &state,
                "p".into(),
                "latest".into(),
                paths,
                Format::Zip,
                out.clone(),
                sums,
                &|_| {},
            )
        };
        let a = export(vec![docs.clone()], Some(Checksums::Sha256)).unwrap();
        let name = a
            .archive
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("p-") && name.ends_with("-docs.zip"),
            "{name}"
        );
        assert!(
            a.manifest
                .unwrap()
                .to_string_lossy()
                .ends_with("-docs.zip.sha256")
        );
        let b = export(vec![docs], None).unwrap();
        assert!(
            b.archive.to_string_lossy().ends_with("-docs-2.zip"),
            "{:?}",
            b.archive
        );
        let all = export(Vec::new(), None).unwrap();
        assert_eq!(all.files, 1);
        assert!(
            export_snapshot_impl(
                &state,
                "p".into(),
                "latest".into(),
                Vec::new(),
                Format::Zip,
                "relative".into(),
                None,
                &|_| {}
            )
            .is_err()
        );
    }
}
