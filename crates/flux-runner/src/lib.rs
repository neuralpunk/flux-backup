//! The job runner: everything around an engine run, shared by the CLI, the scheduler and
//! the GUI.
//!
//! ```text
//! pause, battery and metered checks (scheduled runs) → lower priority (scheduled runs)
//!   → inhibit suspend → pre_backup hooks → engine backup (+ retention)
//!   → post_backup / on_error hooks → upkeep when due (scheduled runs)
//!   → notification (failures; successes if asked) → run record + event log
//! ```

#![deny(clippy::unwrap_used)]

pub mod archive;
pub mod calendar;
pub mod cron;
pub mod desktop;
pub mod index;
pub mod jobs;
pub mod maintenance;
pub mod password;
pub mod prefs;
pub mod recover;
pub mod repo;
pub mod restore_test;
pub mod runlog;
pub mod sample;
pub mod schedule;
pub mod sentry;
pub mod templates;
pub mod tripwire;
pub mod unusual;
pub mod view;
pub mod webhooks;

use std::collections::{BTreeMap, HashSet};

use jiff::Timestamp;

use flux_core::hooks::{self, HookError, HookEvent, HookRun};
use flux_core::profile::{Engine, Profile, Target};
use flux_core::progress::{Event, Progress, Reporter};
use flux_core::{Paths, ProfileId, power};
use flux_fs::outcome::BackupOutcome;
use flux_fs::snapshot::SnapshotStatus;
use flux_mirror::{BackupRequest, MirrorError};
use flux_vault::VaultError;

pub use runlog::{RunRecord, RunStatus, Trigger};

pub struct BackupJob<'a> {
    pub paths: &'a Paths,
    pub id: &'a ProfileId,
    pub profile: &'a Profile,
    pub name: Option<String>,
    pub dry_run: bool,
    pub trigger: Trigger,
    /// Apply retention after the run (and make room before it if needed).
    pub prune: bool,
    /// For interval schedules: do nothing unless the last successful run is older than the
    /// interval.
    pub only_if_due: bool,
    /// How to ask for a Vault password if the configured source doesn't provide one.
    pub ask: Option<password::Ask<'a>>,
    /// Shut the computer down when the backup ends (unless it's stopped). Can be changed
    /// while it runs, through its status socket.
    pub shutdown_after: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Engine(#[from] MirrorError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Repo(#[from] repo::RepoError),
    #[error(transparent)]
    Compat(#[from] flux_compat::CompatError),
    #[error(transparent)]
    Hook(#[from] HookError),
    #[error("skipped: {0}")]
    Skipped(String),
    /// Flux Sentry found unusual changes in the last backup, and they aren't settled:
    /// no backup is made until they are.
    #[error("{0}")]
    Held(String),
    #[error("a backup of this profile is already running (watch it with `flux watch {0}`)")]
    Busy(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{} isn't there: {why}", path.display())]
    SourceMissing {
        path: std::path::PathBuf,
        why: String,
    },
}

impl RunError {
    pub fn is_cancelled(&self) -> bool {
        match self {
            RunError::Engine(MirrorError::Cancelled)
            | RunError::Vault(VaultError::Cancelled)
            | RunError::Compat(flux_compat::CompatError::Cancelled) => true,
            RunError::Repo(r) => r.is_cancelled(),
            _ => false,
        }
    }
}

pub struct RunResult {
    pub record: RunRecord,
    pub outcome: Result<BackupOutcome, RunError>,
}

pub fn run_backup(job: &BackupJob<'_>, progress: &Progress, report: Reporter<'_>) -> RunResult {
    let started = Timestamp::now();
    // Cheap skips first. They are not recorded, since a check every few minutes would
    // flood the history.
    if let Some(why) = quick_skip(job, started) {
        // A scheduled backup held back (paused, battery, network) is worth telling; the
        // frequent "not due yet" checks aren't.
        if !why.starts_with("not due") && !job.dry_run {
            webhooks::send(
                job.paths,
                job.id,
                job.profile,
                flux_core::webhook::HookEvent::Skipped,
                format!("Backup of “{}” skipped: {why}.", job.profile.name),
                (Some("skipped".into()), None, None),
            );
        }
        return unrecorded(
            job,
            started,
            RunStatus::Skipped,
            Err(RunError::Skipped(why)),
        );
    }
    // On hold: Flux Sentry found unusual changes in the last backup and nobody has said
    // what they were. Nothing more is backed up, by schedule or by hand, until someone
    // has: another backup could only store more of the damage, and pass it on to the
    // other places.
    if !job.dry_run
        && job.profile.unusual_changes != flux_core::profile::Sensitivity::Off
        && let Some(u) = sentry_hold(job.paths, job.id)
    {
        let why = format!(
            "backups of “{}” are on hold: Flux Sentry found unusual changes in the last one ({}). Look at them, then say they were yours or that it's dealt with (on the backup's Overview, or `flux sentry {}`)",
            job.profile.name,
            u.signals.join("; "),
            job.id
        );
        if job.trigger == Trigger::Scheduled {
            webhooks::send(
                job.paths,
                job.id,
                job.profile,
                flux_core::webhook::HookEvent::Skipped,
                format!("Backup of “{}” not made: {why}.", job.profile.name),
                (Some("skipped".into()), None, None),
            );
            // Said again once a day for as long as it lasts: backups that silently
            // stopped would be worse than what stopped them.
            if job.profile.options.notify && hold_reminder_due(job.paths, job.id, started) {
                desktop::notify(
                    &format!("Backups of “{}” are on hold", job.profile.name),
                    "Flux Sentry found unusual changes in the last backup, and no backup is made until you've looked. Open Flux Backup to see them.",
                    desktop::Urgency::Critical,
                );
            }
        }
        return unrecorded(job, started, RunStatus::Skipped, Err(RunError::Held(why)));
    }
    let lock = if job.dry_run {
        None
    } else {
        match jobs::try_lock(job.paths, job.id) {
            Ok(Some(l)) => Some(l),
            Ok(None) => {
                return unrecorded(
                    job,
                    started,
                    RunStatus::Skipped,
                    Err(RunError::Busy(job.id.to_string())),
                );
            }
            Err(e) => {
                tracing::warn!("cannot take the job lock: {e}");
                None
            }
        }
    };

    let log = runlog::RunLog::start(job.paths, job.id, started);
    if !job.dry_run {
        for (url, e) in webhooks::send(
            job.paths,
            job.id,
            job.profile,
            flux_core::webhook::HookEvent::Started,
            format!("Backup of “{}” started.", job.profile.name),
            (Some("running".into()), None, None),
        ) {
            log.event(&Event::Warning {
                path: String::new(),
                message: format!("webhook {url}: {e}"),
            });
        }
    }
    let server = lock.as_ref().and_then(|_| {
        let info = jobs::JobInfo {
            profile: job.id.to_string(),
            run_id: log.run_id.clone(),
            pid: std::process::id(),
            started,
            trigger: job.trigger,
            socket: Default::default(),
        };
        jobs::bind(job.paths, job.id, info)
            .map_err(|e| tracing::warn!("status socket unavailable: {e}"))
            .ok()
    });
    let shared = server
        .as_ref()
        .map(|(_, info)| jobs::Shared::new(info.clone(), progress));
    if let Some(sh) = &shared {
        sh.set_shutdown_after(job.shutdown_after);
    }
    let tee = |ev: Event| {
        log.event(&ev);
        if let Some(sh) = &shared {
            sh.push_event(&ev);
        }
        report(ev);
    };

    let (outcome, record) = std::thread::scope(|scope| {
        if let (Some((srv, _)), Some(sh)) = (&server, &shared) {
            scope.spawn(move || srv.serve(scope, sh));
        }
        // Whatever ends this run, a panic included, the status server is told to stop:
        // the scope waits for it, and would wait for ever.
        struct StopServer<'a, 'b>(Option<&'a jobs::Shared<'b>>);
        impl Drop for StopServer<'_, '_> {
            fn drop(&mut self) {
                if let Some(sh) = self.0 {
                    sh.shutdown();
                }
            }
        }
        let _stop_server = StopServer(shared.as_ref());
        let mut hooks_run: Vec<HookRun> = Vec::new();
        let mut sources_missing: Vec<std::path::PathBuf> = Vec::new();
        let mut destinations: Vec<runlog::DestinationRun> = Vec::new();
        let mut unusual: Option<unusual::UnusualReport> = None;
        let mut sentry_seen: Option<sentry::Run> = None;
        let turn = wait_for_turn(job, progress, &tee);
        let outcome = match &turn {
            Err(()) => Err(RunError::Engine(MirrorError::Cancelled)),
            Ok(_) => run_inner(
                job,
                progress,
                &tee,
                &mut hooks_run,
                &mut sources_missing,
                &mut destinations,
                &mut unusual,
                &mut sentry_seen,
            ),
        };
        if job.trigger == Trigger::Scheduled
            && !job.dry_run
            && matches!(
                outcome,
                Ok(BackupOutcome::Created { .. } | BackupOutcome::Unchanged { .. })
            )
        {
            maintenance::after_backup(job.paths, job.id, job.profile, job.ask, progress, &tee);
        }
        // A restore test, when one is due.
        if !job.dry_run
            && matches!(
                outcome,
                Ok(BackupOutcome::Created { .. } | BackupOutcome::Unchanged { .. })
            )
            && !progress.is_cancelled()
            && restore_test::due(
                job.paths,
                job.id,
                &job.profile.restore_test,
                Timestamp::now(),
            )
        {
            // The test reuses the run's progress: its own phases aren't the backup's.
            progress.set_phase(flux_core::progress::Phase::Done);
            let phases = progress.timings.phases();
            let t = std::time::Instant::now();
            restore_test_after(job, progress, &tee);
            progress.set_phase(flux_core::progress::Phase::Done);
            progress.timings.restore_phases(phases);
            progress
                .timings
                .record_phase("Testing a restore", t.elapsed().as_secs_f64());
        }
        let mut record = finish_run(job, started, &log.run_id, progress, &outcome, hooks_run);
        record.unusual = unusual;
        record.sentry_seen = sentry_seen;
        record.sources_missing = sources_missing;
        record.destinations = destinations;
        if !job.dry_run {
            for (url, e) in tell_webhooks(job, &record) {
                tee(Event::Warning {
                    path: String::new(),
                    message: format!("webhook {url}: {e}"),
                });
            }
        }
        drop(turn);
        let wants = shared
            .as_ref()
            .map_or(job.shutdown_after, |s| s.shutdown_after());
        let shutdown = wants
            && !job.dry_run
            && record.status != RunStatus::Cancelled
            && power_off_soon(job, &tee);
        if let Some(sh) = &shared {
            sh.finish(record.status, record.error.clone(), shutdown);
            sh.shutdown();
        }
        (outcome, record)
    });
    if !job.dry_run {
        log.finish(&record);
        let r = &job.profile.retention;
        if r.keep_history_days.is_some() || r.keep_history_runs.is_some() {
            runlog::trim(
                job.paths,
                job.id,
                r.keep_history_days,
                r.keep_history_runs,
                Timestamp::now(),
            );
        }
    }
    drop(lock);
    RunResult { record, outcome }
}

/// What went wrong in the last backup that ran (not skipped or stopped), in words, if
/// anything did: failed, finished with problems, a source's drive missing, a destination
/// failing.
fn last_problem(job: &BackupJob<'_>) -> Option<String> {
    let last = runlog::recent_runs(job.paths, job.id, 10)
        .into_iter()
        .find(|r| {
            !matches!(
                r.status,
                RunStatus::Skipped | RunStatus::DryRun | RunStatus::Cancelled
            )
        })?;
    match last.status {
        RunStatus::Failed => Some("failed".into()),
        RunStatus::CompletedWithErrors => Some("couldn't back up some files".into()),
        _ if last.unusual.is_some() && !unusual_settled(job.paths, job.id, &last) => {
            Some("found unusual changes".into())
        }
        _ if !last.sources_missing.is_empty() => Some("missed a source's drive".into()),
        _ => last
            .destinations
            .iter()
            .find(|d| d.status == RunStatus::Failed)
            .map(|d| format!("failed at {}", d.name)),
    }
}

/// Check that every source's drive is there. Sources set to wait are waited for (the
/// job shows as waiting and can be stopped); a source set to fail, or waited for in vain,
/// fails the run; the rest are left out, and said so. Returns the profile with only the
/// sources that are there, and the paths left out (Mirror and Vault keep their files from
/// the previous snapshot). Nothing left to back up skips the run.
fn sources_ready(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<(flux_core::Profile, Vec<std::path::PathBuf>, bool), RunError> {
    use flux_core::profile::WhenMissing;
    use flux_core::source_check::missing_sources;
    let mut missing = missing_sources(job.profile);
    let waits = |m: &[flux_core::source_check::MissingSource]| {
        m.iter()
            .filter(|s| s.when_missing == WhenMissing::Wait)
            .map(|s| s.wait_for)
            .max()
    };
    if let Some(longest) = waits(&missing)
        && !job.dry_run
    {
        let deadline = std::time::Instant::now() + longest;
        let poll = (longest / 8).clamp(
            std::time::Duration::from_millis(200),
            std::time::Duration::from_secs(15),
        );
        progress.set_phase(flux_core::progress::Phase::Waiting);
        for m in missing
            .iter()
            .filter(|m| m.when_missing == WhenMissing::Wait)
        {
            report(Event::Info {
                message: format!(
                    "waiting for {} ({}), up to {}",
                    m.path.display(),
                    m.why,
                    flux_core::HumanDuration(m.wait_for)
                ),
            });
        }
        while waits(&missing).is_some() && std::time::Instant::now() < deadline {
            if progress.is_cancelled() {
                return Err(RunError::Engine(MirrorError::Cancelled));
            }
            std::thread::sleep(poll);
            missing = missing_sources(job.profile);
        }
        progress.set_phase(flux_core::progress::Phase::Preparing);
        if waits(&missing).is_none() {
            report(Event::Info {
                message: "the drive is connected; starting".into(),
            });
        }
    }
    let mut keep = Vec::new();
    for m in &missing {
        match m.when_missing {
            WhenMissing::Fail => {
                return Err(RunError::SourceMissing {
                    path: m.path.clone(),
                    why: m.why.clone(),
                });
            }
            WhenMissing::Wait if !job.dry_run => {
                return Err(RunError::SourceMissing {
                    path: m.path.clone(),
                    why: format!(
                        "{}; waited {} for it",
                        m.why,
                        flux_core::HumanDuration(m.wait_for)
                    ),
                });
            }
            WhenMissing::Wait | WhenMissing::Skip => keep.push(m.path.clone()),
        }
    }
    if keep.is_empty() {
        return Ok((job.profile.clone(), keep, false));
    }
    if keep.len() == job.profile.sources.paths.len() {
        return Err(RunError::Skipped(format!(
            "nothing to back up: {} isn't connected",
            flux_mirror::list_paths(&keep)
        )));
    }
    let carries = engine_keeps(job.profile);
    for m in missing.iter().filter(|m| keep.contains(&m.path)) {
        report(Event::Warning {
            path: m.path.display().to_string(),
            message: format!(
                "{}; {}",
                m.why,
                if carries {
                    "its files are kept as they were in the last backup"
                } else {
                    "it's left out of this snapshot (earlier snapshots still have it)"
                }
            ),
        });
        progress
            .warnings
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let mut reduced = job.profile.clone();
    reduced.sources.paths.retain(|p| !keep.contains(p));
    Ok((reduced, keep, carries))
}

/// Tell the webhooks how a run went: its result (a warning when files were left out, a
/// source's drive was missing or a place failed), unusual changes, and low space.
fn tell_webhooks(job: &BackupJob<'_>, r: &RunRecord) -> Vec<(String, String)> {
    use flux_core::webhook::HookEvent as E;
    let name = &job.profile.name;
    let problems: Vec<String> = r
        .sources_missing
        .iter()
        .map(|p| format!("{} wasn't connected", p.display()))
        .chain(
            r.destinations
                .iter()
                .filter(|d| d.status == RunStatus::Failed)
                .map(|d| format!("the copy on {} failed", d.name)),
        )
        .collect();
    // A backup Flux Sentry stopped: that, and nothing about a backup.
    if r.status == RunStatus::Skipped
        && let Some(u) = &r.unusual
    {
        return webhooks::send(
            job.paths,
            job.id,
            job.profile,
            E::UnusualChanges,
            format!(
                "Flux Sentry stopped the backup of “{name}”: unusual changes in the files: {}. Backups are on hold.",
                u.signals.join("; ")
            ),
            (Some("skipped".into()), None, None),
        );
    }
    let (event, message) = match r.status {
        RunStatus::Failed => (
            E::Failed,
            format!(
                "Backup of “{name}” failed: {}",
                r.error.as_deref().unwrap_or("unknown error")
            ),
        ),
        RunStatus::CompletedWithErrors => (
            E::Warning,
            format!(
                "Backup of “{name}” finished, but {} files couldn't be backed up.",
                r.warnings
            ),
        ),
        RunStatus::Completed | RunStatus::Unchanged if !problems.is_empty() => (
            E::Warning,
            format!(
                "Backup of “{name}” finished with problems: {}.",
                problems.join("; ")
            ),
        ),
        RunStatus::Completed => (
            E::Succeeded,
            format!(
                "Backup of “{name}” finished: snapshot {}{}.",
                r.snapshot.as_deref().unwrap_or("-"),
                r.stats
                    .as_ref()
                    .map(|s| format!(", {} files new or changed", s.new + s.changed))
                    .unwrap_or_default()
            ),
        ),
        RunStatus::Unchanged => (
            E::Succeeded,
            format!("Backup of “{name}”: nothing changed since the last one."),
        ),
        RunStatus::Skipped | RunStatus::Cancelled | RunStatus::DryRun => return Vec::new(),
    };
    let status = format!("{:?}", r.status).to_lowercase();
    let mut failed = webhooks::send(
        job.paths,
        job.id,
        job.profile,
        event,
        message,
        (Some(status), r.snapshot.clone(), r.error.clone()),
    );
    if let Some(u) = &r.unusual {
        failed.extend(webhooks::send(
            job.paths,
            job.id,
            job.profile,
            E::UnusualChanges,
            format!(
                "Unusual changes in the backup of “{name}”: {}.",
                u.signals.join("; ")
            ),
            (None, Some(u.snapshot.clone()), None),
        ));
    }
    if let (Some(free), Some(size)) = (r.free_after, disk_size(&job.profile.target))
        && size > 0
        && (free * 20 < size || free < 1 << 30)
    {
        failed.extend(webhooks::send(
            job.paths,
            job.id,
            job.profile,
            E::LowSpace,
            format!(
                "Little space left where “{name}” is backed up: {} free.",
                flux_core::ByteSize(free).display_approx()
            ),
            (None, None, None),
        ));
    }
    failed
}

/// Look for unusual changes in snapshot `snapshot` against the one before. When found:
/// tag it, pin the one before (with a note), say so and notify.
/// `left_out`: sources that aren't in this snapshot because their drive isn't connected
/// (and whose files weren't carried over): their absence is not a deletion.
/// `seen`: what the look counted, when there was one.
fn check_unusual(
    job: &BackupJob<'_>,
    snapshot: &str,
    left_out: &[std::path::PathBuf],
    seen: &mut Option<sentry::Run>,
    report: Reporter<'_>,
) -> Option<unusual::UnusualReport> {
    let sens = job.profile.unusual_changes;
    if sens == flux_core::profile::Sensitivity::Off {
        return None;
    }
    let repo = repo::AnyRepo::open(job.paths, job.id, job.profile, job.ask).ok()?;
    let snaps = repo.snapshots().ok()?;
    let i = snaps.iter().position(|s| s.id == snapshot)?;
    // The snapshot to compare with is the newest one that isn't itself marked as
    // unusual. After an alarm that was dealt with (the files restored), the snapshot
    // right before this one is the one with the damage in it: against that, the clean
    // backup would look like another attack, and the damaged one would be pinned as
    // "the one from before". ("This was me" takes the mark off, and it counts again.)
    let prev = snaps[..i]
        .iter()
        .rev()
        .find(|s| !s.tags.iter().any(|t| t == UNUSUAL_TAG))?
        .clone();
    let now = snaps[i].clone();
    // Both lists streamed side by side (they are sorted by path): only what differs is
    // kept, however many files there are.
    let skip = roots_of(left_out);
    let d = {
        let failed = std::cell::Cell::new(false);
        let files = |s: &flux_fs::snapshot::SnapshotMeta| {
            let failed = &failed;
            Some(
                repo.entries(s)
                    .ok()?
                    .map_while(move |e| e.inspect_err(|_| failed.set(true)).ok()),
            )
        };
        let before = files(&prev)?.filter(|e| !under(&e.path, &skip));
        let d = match unusual::diff(before, files(&now)?) {
            Some(d) => d,
            // A listing that isn't in path order (another tool's): sorted first.
            None => {
                let sorted = |s: &flux_fs::snapshot::SnapshotMeta| {
                    let mut v: Vec<flux_fs::entry::Entry> = files(s)?
                        .filter(|e| e.kind == flux_fs::entry::Kind::File)
                        .collect();
                    v.sort_by(|a, b| a.path.cmp(&b.path));
                    v.dedup_by(|a, b| a.path == b.path);
                    Some(v)
                };
                let before = sorted(&prev)?
                    .into_iter()
                    .filter(|e| !under(&e.path, &skip));
                unusual::diff(before, sorted(&now)?.into_iter())?
            }
        };
        if failed.get() {
            return None;
        }
        d
    };
    let history = sentry_history(
        job.paths,
        job.id,
        runlog::recent_runs(job.paths, job.id, SENTRY_READ),
    );
    let (counted, found) = unusual::analyze_diff(&d, &history, sens, &read_start);
    *seen = counted;
    let mut r = found?;
    r.snapshot = now.id.clone();
    r.before = Some(prev.id.clone());
    // Mark the new snapshot; keep the one before for sure.
    let mut tags = now.tags.clone();
    tags.push(UNUSUAL_TAG.into());
    if let Err(e) = repo.change(
        &now.id,
        &flux_fs::snapshot::SnapshotChange {
            tags: Some(tags),
            ..Default::default()
        },
    ) {
        report(Event::Warning {
            path: String::new(),
            message: format!(
                "snapshot {} couldn't be marked as having unusual changes: {e}",
                now.id
            ),
        });
    }
    (r.before_pinned, r.pinned_by_flux) = raise_alarm(job, Some((&repo, &prev)), &r, report);
    Some(r)
}

/// `paths` as snapshot entries name them (no leading slash).
fn roots_of(paths: &[std::path::PathBuf]) -> Vec<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    paths
        .iter()
        .map(|p| {
            let b = p.as_os_str().as_bytes();
            b.strip_prefix(b"/").unwrap_or(b).to_vec()
        })
        .collect()
}

/// Is `path` (a snapshot entry's) one of `roots` or below one?
fn under(path: &[u8], roots: &[Vec<u8>]) -> bool {
    roots.iter().any(|r| flux_fs::entry::is_within(path, r))
}

/// The start of a file as it is now (for the "looks encrypted" signal).
fn read_start(p: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(64 << 10);
    std::fs::File::open(p)
        .ok()?
        .take(64 << 10)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// Unusual changes were found: pin the snapshot from before them (with a note), say so
/// in the log and by a notification. Returns whether that snapshot is pinned now, and
/// whether Flux pinned it (it wasn't already).
fn raise_alarm(
    job: &BackupJob<'_>,
    kept: Option<(&repo::AnyRepo, &flux_fs::snapshot::SnapshotMeta)>,
    r: &unusual::UnusualReport,
    report: Reporter<'_>,
) -> (bool, bool) {
    for s in &r.signals {
        report(Event::Warning {
            path: String::new(),
            message: format!("unusual changes: {s}"),
        });
    }
    let Some((repo, prev)) = kept else {
        // No snapshot yet to keep.
        if job.profile.options.notify {
            desktop::notify(
                &format!("Unusual changes in “{}”", job.profile.name),
                &format!(
                    "{}. The backup wasn't made, and backups are on hold. If you didn't make these changes, check your files.",
                    r.signals.join("; ")
                ),
                desktop::Urgency::Critical,
            );
        }
        return (false, false);
    };
    let day = prev
        .started
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d %H:%M")
        .to_string();
    // Pinned, or said not to be: "kept for sure" must be true.
    let mut by_flux = false;
    let pinned = prev.pinned
        || match repo.change(
            &prev.id,
            &flux_fs::snapshot::SnapshotChange {
                pinned: Some(true),
                note: prev
                    .note
                    .is_none()
                    .then(|| Some(format!("{AUTO_PIN_NOTE} ({day})."))),
                ..Default::default()
            },
        ) {
            Ok(_) => {
                by_flux = true;
                true
            }
            Err(e) => {
                report(Event::Warning {
                    path: String::new(),
                    message: format!(
                        "snapshot {}, the last one before these changes, couldn't be pinned ({e}): pin it yourself so it isn't removed",
                        prev.id
                    ),
                });
                false
            }
        };
    if pinned {
        report(Event::Info {
            message: format!(
                "kept snapshot {} for sure (pinned): the last one before these changes",
                prev.id
            ),
        });
    }
    if job.profile.options.notify {
        desktop::notify(
            &format!("Unusual changes in “{}”", job.profile.name),
            &format!(
                "{}. {} The snapshot from {day}, before them, {}. If you didn't make these changes, don't delete anything and check your files.",
                r.signals.join("; "),
                if r.stopped {
                    "The backup wasn't made, and backups are on hold."
                } else {
                    "Backups are on hold."
                },
                if pinned {
                    "is kept"
                } else {
                    "couldn't be pinned: open Flux and pin it"
                }
            ),
            desktop::Urgency::Critical,
        );
    }
    (pinned, by_flux)
}

/// What a look at the files before a backup found.
enum Looked {
    /// Nothing unusual: the backup can be written (and needn't be looked at after).
    /// With what the look counted, unless the owner's word stood in for one.
    /// And the scan it was made from, for the backup to use.
    Fine(Option<sentry::Run>, Option<flux_fs::scan::ScanResult>),
    /// Unusual changes: the backup isn't made.
    Unusual(Box<unusual::UnusualReport>),
    /// It couldn't be had (no snapshot to compare with, backups written by another
    /// tool…): the backup is made and looked at afterwards. With the scan, if one was
    /// made before that turned out.
    CouldNot(Option<flux_fs::scan::ScanResult>),
}

/// The owner settled what stopped the last backup (the changes were theirs, or they've
/// dealt with them), and no backup has been made since: this one is theirs to make,
/// without being looked at again. It would be stopped for the very same changes, or for
/// what an attack left lying about (renamed files, ransom notes) after the files were
/// restored. The snapshot from before stays pinned either way.
fn owner_cleared(paths: &flux_core::Paths, id: &flux_core::ProfileId) -> bool {
    runlog::recent_runs(paths, id, 50)
        .iter()
        .find(|r| {
            r.unusual.is_some()
                || matches!(
                    r.status,
                    RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Unchanged
                )
        })
        .is_some_and(|r| {
            r.unusual.as_ref().is_some_and(|u| u.stopped) && unusual_settled(paths, id, r)
        })
}

/// Look at the sources as they are now against the newest snapshot, before a backup is
/// written. For Flux's own formats (Mirror and Vault), whose snapshots record files just
/// as a scan sees them. `keep`: sources whose drive isn't there, kept as they were.
fn look_before(
    job: &BackupJob<'_>,
    keep: &[std::path::PathBuf],
    left_out: &[std::path::PathBuf],
    progress: &Progress,
    report: Reporter<'_>,
) -> Looked {
    let p = job.profile;
    let sens = p.unusual_changes;
    if sens == flux_core::profile::Sensitivity::Off {
        return Looked::CouldNot(None);
    }
    if owner_cleared(job.paths, job.id) {
        report(Event::Info {
            message: "Flux Sentry: you settled the changes that stopped the last backup; backing up without another look".into(),
        });
        return Looked::Fine(None, None);
    }
    // The tripwires first: for a backup of any kind, and whatever else has changed.
    let tripped = if p.tripwires {
        tripwire::check(job.paths, job.id, &p.sources.paths)
    } else {
        Vec::new()
    };
    let repo = repo::AnyRepo::open(job.paths, job.id, p, job.ask).ok();
    let prev = repo
        .as_ref()
        .and_then(|r| r.snapshots().ok())
        .and_then(|mut s| s.pop());
    // The files against the newest snapshot, where a scan can be compared with one.
    // The scan is the backup's own, made here instead of there: the folders are walked
    // (and, with checksums, the files read) once, for the look and the backup both.
    let mut scan = None;
    let compared = match (&repo, &prev) {
        (Some(repo), Some(prev))
            if matches!(p.engine, Engine::Mirror | Engine::Vault) && tripped.is_empty() =>
        {
            scan = scan_as_engine(job, progress, report);
            scan.as_ref()
                .and_then(|s| compare_with(job, repo, prev, &s.entries, keep, left_out))
        }
        // A tripwire has spoken: the backup is stopped whatever the files say, and they
        // are looked at the way that costs nothing more.
        (Some(repo), Some(prev)) if matches!(p.engine, Engine::Mirror | Engine::Vault) => {
            scan_for_look(job).and_then(|s| compare_with(job, repo, prev, &s, keep, left_out))
        }
        _ => None,
    };
    let found = match &compared {
        Some((_, found)) => unusual::with_tripwires(found.clone(), &tripped),
        None => unusual::with_tripwires(None, &tripped),
    };
    let Some(mut r) = found else {
        return match compared {
            Some((seen, _)) => Looked::Fine(seen, scan),
            None => Looked::CouldNot(scan),
        };
    };
    r.stopped = true;
    r.snapshot = format!("before:{}", Timestamp::now());
    r.before = prev.as_ref().map(|s| s.id.clone());
    report(Event::Warning {
        path: String::new(),
        message: "Flux Sentry stopped this backup before anything was written".into(),
    });
    (r.before_pinned, r.pinned_by_flux) =
        raise_alarm(job, repo.as_ref().zip(prev.as_ref()), &r, report);
    Looked::Unusual(Box::new(r))
}

/// The backup's scan of the sources, made before the backup starts: with the engine's
/// own exclusions, counted in the run's progress, its warnings reported. `None` if the
/// scan failed (the backup then scans for itself, and says why it can't) or this kind
/// of backup doesn't scan.
fn scan_as_engine(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Option<flux_fs::scan::ScanResult> {
    let p = job.profile;
    match p.engine {
        Engine::Mirror if matches!(p.target, Target::Ssh { .. }) => {
            flux_mirror::remote::scan_for_backup(p, progress, report).ok()
        }
        Engine::Mirror => flux_mirror::scan_for_backup(job.id, p, progress, report).ok(),
        Engine::Vault => {
            flux_vault::backup::scan_for_backup(p, &vault_never(p), progress, report).ok()
        }
        Engine::Rsync | Engine::Borg => None,
    }
}

/// A scan of the sources for a look alone (no backup will use it): quietly.
fn scan_for_look(job: &BackupJob<'_>) -> Option<Vec<flux_fs::entry::Entry>> {
    let p = job.profile;
    // The backups themselves are never backed up.
    let never: Vec<std::path::PathBuf> = match &p.target {
        Target::Local { path, .. } => vec![path.join("flux-backup"), path.join("flux-vault")],
        Target::Ssh { .. } => Vec::new(),
    };
    let scanned = flux_fs::scan::thread_pool(p).ok()?.install(|| {
        flux_fs::scan::scan_profile(p, &never, &Progress::new(), &flux_core::progress::silent)
    });
    Some(scanned.ok()?.entries)
}

/// What a Vault backup never scans: the Vault itself, and a plain-folder backup beside it.
fn vault_never(p: &flux_core::Profile) -> Vec<std::path::PathBuf> {
    match &p.target {
        Target::Local { .. } => {
            let root = repo::vault_root(p);
            vec![root.clone(), root.with_file_name("flux-backup")]
        }
        Target::Ssh { .. } => Vec::new(),
    }
}

/// The sources as scanned just now (`after`, sorted by path) against snapshot `prev`:
/// `None` if the look couldn't be had; otherwise what it counted and, if anything is
/// unusual, what. `keep`: sources whose drive is away and whose files the backup carries
/// over from `prev`; `left_out`: those it leaves out of the snapshot instead (their
/// absence is not a deletion).
fn compare_with(
    job: &BackupJob<'_>,
    repo: &repo::AnyRepo,
    prev: &flux_fs::snapshot::SnapshotMeta,
    after: &[flux_fs::entry::Entry],
    keep: &[std::path::PathBuf],
    left_out: &[std::path::PathBuf],
) -> Option<(Option<sentry::Run>, Option<unusual::UnusualReport>)> {
    let p = job.profile;
    // Sources whose drive is away are in neither list: the scan doesn't see them, and
    // the snapshot's are passed over (carried over or left out, they aren't changes).
    let mut away = roots_of(keep);
    away.extend(roots_of(left_out));
    // The snapshot's list streams by against the scan (both are sorted by path): only
    // what differs is kept. A Vault's comes from the list the last backup left in the
    // cache, when it is this snapshot's: reading it from the Vault is a read per
    // folder, hours over a network.
    let failed = std::cell::Cell::new(false);
    let before = repo
        .entries_for_look(job.paths, prev)
        .ok()?
        .map_while(|e| e.inspect_err(|_| failed.set(true)).ok())
        .filter(|e| !under(&e.path, &away))
        // A scan has no checksums: compare by size and time, as the scan can.
        .map(|mut e| {
            e.hash = None;
            e
        });
    let d = unusual::diff(before, after.iter().cloned())?;
    if failed.get() {
        return None;
    }
    let history = sentry_history(
        job.paths,
        job.id,
        runlog::recent_runs(job.paths, job.id, SENTRY_READ),
    );
    Some(unusual::analyze_diff(
        &d,
        &history,
        p.unusual_changes,
        &read_start,
    ))
}

/// The tag a snapshot with unusual changes gets.
pub const UNUSUAL_TAG: &str = "unusual changes";
/// The note on the snapshot pinned because unusual changes came after it.
pub const AUTO_PIN_NOTE: &str = "Kept by Flux: the last snapshot before unusual changes";

fn unusual_state(paths: &flux_core::Paths, id: &flux_core::ProfileId) -> std::path::PathBuf {
    paths
        .state_dir
        .join("unusual")
        .join(format!("{id}.dismissed"))
}

/// How many of a backup's latest runs Flux Sentry learns from.
pub const SENTRY_RUNS: usize = 60;
/// How many run records are read to find them.
pub const SENTRY_READ: usize = 300;

/// The runs Flux Sentry learns a backup's normal from (`runs`: its latest, newest
/// first): not those whose changes were found unusual, unless their owner said they were
/// theirs. What looked like an attack isn't taken for a habit.
pub fn sentry_history(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    runs: Vec<RunRecord>,
) -> Vec<RunRecord> {
    let dismissed = dismissed_unusual(paths, id);
    // The last 60 that made a snapshot: failed, skipped and unchanged runs in between
    // (a drive away for a few days of an hourly backup) don't push them out.
    // The first backup after a stopped one that was *dealt with* is made without a look
    // (see `owner_cleared`) and may hold whatever of the changes wasn't put back:
    // "nothing is learned" covers it too.
    let resolved = lines_of(&resolved_state(paths, id));
    let makes = |r: &RunRecord| {
        matches!(
            r.status,
            RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Unchanged
        )
    };
    let unlooked: HashSet<String> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| makes(r) && r.unusual.is_none())
        .filter(|(i, _)| {
            runs[i + 1..]
                .iter()
                .find(|older| older.unusual.is_some() || makes(older))
                .and_then(|older| older.unusual.as_ref())
                .is_some_and(|u| {
                    u.stopped && resolved.contains(&u.snapshot) && !dismissed.contains(&u.snapshot)
                })
        })
        .map(|(_, r)| r.run_id.clone())
        .collect();
    let mut made = 0;
    runs.into_iter()
        .filter(|r| !unlooked.contains(&r.run_id))
        .take_while(|r| {
            let counts = matches!(
                r.status,
                RunStatus::Completed | RunStatus::CompletedWithErrors
            );
            made += usize::from(counts);
            made <= SENTRY_RUNS
        })
        .filter(|r| {
            r.unusual
                .as_ref()
                .is_none_or(|u| dismissed.contains(&u.snapshot))
        })
        .collect()
}

/// The snapshots whose unusual changes were dismissed, oldest first (the last 100).
fn dismissed_unusual(paths: &flux_core::Paths, id: &flux_core::ProfileId) -> Vec<String> {
    std::fs::read_to_string(unusual_state(paths, id))
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn resolved_state(paths: &flux_core::Paths, id: &flux_core::ProfileId) -> std::path::PathBuf {
    paths
        .state_dir
        .join("unusual")
        .join(format!("{id}.resolved"))
}

fn lines_of(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The person said the unusual changes in `snapshot` are dealt with (files restored, the
/// computer cleaned): backups go on. Unlike [`dismiss_unusual`], the changes weren't
/// theirs, so Flux Sentry learns nothing from that backup, and the snapshot before it
/// stays pinned.
pub fn resolve_unusual(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    snapshot: &str,
) -> std::io::Result<()> {
    let p = resolved_state(paths, id);
    std::fs::create_dir_all(p.parent().expect("state folder"))?;
    let mut all = lines_of(&p);
    all.retain(|s| s != snapshot);
    all.push(snapshot.to_owned());
    let from = all.len().saturating_sub(100);
    flux_core::store::write_atomic(&p, all[from..].join("\n").as_bytes(), 0o600)
        .map_err(std::io::Error::other)
}

/// Were the unusual changes of this run settled: said to be the person's own, or dealt
/// with?
pub fn unusual_settled(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    run: &RunRecord,
) -> bool {
    unusual_dismissed(paths, id, run)
        || run
            .unusual
            .as_ref()
            .is_some_and(|u| lines_of(&resolved_state(paths, id)).contains(&u.snapshot))
}

/// The unusual changes that hold this backup: those of its latest flagged backup, while
/// they aren't settled.
pub fn sentry_hold(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
) -> Option<unusual::UnusualReport> {
    open_unusual(paths, id, &runlog::recent_runs(paths, id, 300))
}

/// [`sentry_hold`], from runs already read (newest first).
pub fn open_unusual(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    runs: &[RunRecord],
) -> Option<unusual::UnusualReport> {
    // The latest run that found something or made a backup. A backup made since an
    // alarm (Flux Sentry was switched off for a while, say) means it is no longer what
    // holds this backup: an old alarm doesn't come back when Sentry is switched on.
    let run = runs.iter().find(|r| {
        r.unusual.is_some()
            || matches!(
                r.status,
                RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Unchanged
            )
    })?;
    run.unusual.as_ref()?;
    (!unusual_settled(paths, id, run))
        .then(|| run.unusual.clone())
        .flatten()
}

/// A profile was deleted: what Flux keeps for it outside its backups goes or is settled,
/// so that nothing of it clings to a profile later made under the same id. Its tripwire
/// files are removed (those still as planted), its file index is dropped, and an alarm
/// still open is closed as dealt with (the new profile would start out on hold).
/// The run history stays.
pub fn forget_profile(paths: &flux_core::Paths, id: &flux_core::ProfileId) {
    tripwire::remove(paths, id);
    index::remove(paths, id);
    if let Some(u) = sentry_hold(paths, id) {
        let _ = resolve_unusual(paths, id, &u.snapshot);
    }
}

/// Is it time to say again that this backup is on hold? Once a day; saying so counts.
fn hold_reminder_due(paths: &flux_core::Paths, id: &flux_core::ProfileId, now: Timestamp) -> bool {
    let stamp = paths
        .state_dir
        .join("unusual")
        .join(format!("{id}.reminded"));
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse::<Timestamp>().ok());
    // (A stamp from the future, after the clock was set back, doesn't silence it.)
    if last.is_some_and(|l| (0..86_400).contains(&now.duration_since(l).as_secs())) {
        return false;
    }
    // (The folder isn't there until something has been settled once: without it the
    // stamp was never written, and the reminder came with every attempt.)
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = flux_core::store::write_atomic(&stamp, now.to_string().as_bytes(), 0o600);
    true
}

/// Settle the unusual changes of `snapshot` and let backups go on. `mine`: they were the
/// person's own ("This was me"): Flux Sentry counts that backup as normal from now on,
/// its tag goes, and the snapshot before it is unpinned if Flux pinned it. Otherwise
/// they are dealt with: nothing is learned, and the tag and the pin stay.
pub fn settle_unusual(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    profile: &flux_core::Profile,
    snapshot: &str,
    mine: bool,
    repo: Option<&repo::AnyRepo>,
) -> std::io::Result<Settled> {
    // Tripwires that were changed or removed are set again: what tripped them is
    // settled, and they would only stop the next backup too.
    if profile.tripwires {
        tripwire::rearm(paths, id, profile);
    }
    if !mine {
        resolve_unusual(paths, id, snapshot)?;
        return Ok(Settled { marks_left: false });
    }
    dismiss_unusual(paths, id, snapshot)?;
    let alarm = runlog::recent_runs(paths, id, 300)
        .into_iter()
        .find_map(|r| r.unusual.filter(|u| u.snapshot == snapshot));
    // What "mine" takes off again: the tag on the snapshot with the changes (there is
    // none when the backup was stopped before it was written), and the pin Flux put on
    // the one before.
    let tagged = alarm.as_ref().is_some_and(|u| !u.stopped);
    let before = alarm.as_ref().and_then(|u| u.before.clone());
    let Some(snaps) = repo.and_then(|r| r.snapshots().ok().map(|s| (r, s))) else {
        // The backups can't be reached now: the tag and the pin stay as they are.
        return Ok(Settled {
            marks_left: tagged || before.is_some(),
        });
    };
    let (repo, snaps) = snaps;
    let mut marks_left = false;
    if let Some(s) = snaps.iter().find(|s| s.id == snapshot)
        && s.tags.iter().any(|t| t == UNUSUAL_TAG)
    {
        let tags: Vec<String> = s
            .tags
            .iter()
            .filter(|t| *t != UNUSUAL_TAG)
            .cloned()
            .collect();
        marks_left |= repo
            .change(
                &s.id,
                &flux_fs::snapshot::SnapshotChange {
                    tags: Some(tags),
                    ..Default::default()
                },
            )
            .is_err();
    }
    if let Some(s) = before.and_then(|b| snaps.iter().find(|s| s.id == b))
        && s.pinned
    {
        // Flux's own note goes with its pin; a note of the owner's stays.
        let auto_note = s
            .note
            .as_deref()
            .is_some_and(|n| n.starts_with(AUTO_PIN_NOTE));
        if alarm.as_ref().is_some_and(|u| u.pinned_by_flux) || auto_note {
            marks_left |= repo
                .change(
                    &s.id,
                    &flux_fs::snapshot::SnapshotChange {
                        pinned: Some(false),
                        note: auto_note.then_some(None),
                        ..Default::default()
                    },
                )
                .is_err();
        }
    }
    Ok(Settled { marks_left })
}

/// How settling an alarm went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Settled {
    /// "This was me" couldn't take the *unusual changes* tag or Flux's pin off the
    /// snapshots (the backups can't be reached now, or the change failed): they stay
    /// until taken off by hand.
    pub marks_left: bool,
}

/// The person said the unusual changes in `snapshot` were theirs.
pub fn dismiss_unusual(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    snapshot: &str,
) -> std::io::Result<()> {
    let p = unusual_state(paths, id);
    std::fs::create_dir_all(p.parent().expect("state folder"))?;
    // Every one is remembered (before 1.1, only the latest): Flux Sentry learns from
    // the changes that were the owner's.
    let mut all = dismissed_unusual(paths, id);
    all.retain(|s| s != snapshot);
    all.push(snapshot.to_owned());
    let from = all.len().saturating_sub(100);
    flux_core::store::write_atomic(&p, all[from..].join("\n").as_bytes(), 0o600)
        .map_err(std::io::Error::other)
}

/// Were the unusual changes of this run dismissed ("This was me")?
pub fn unusual_dismissed(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    run: &RunRecord,
) -> bool {
    run.unusual
        .as_ref()
        .is_some_and(|u| dismissed_unusual(paths, id).contains(&u.snapshot))
}

/// Run the profile's restore test after a backup: said in the run's log, and notified
/// when it fails.
fn restore_test_after(job: &BackupJob<'_>, progress: &Progress, report: Reporter<'_>) {
    report(Event::Info {
        message: "testing a restore (it's due)".into(),
    });
    let repo = match repo::AnyRepo::open(job.paths, job.id, job.profile, job.ask) {
        Ok(r) => r,
        Err(e) => {
            report(Event::Warning {
                path: String::new(),
                message: format!("couldn't test a restore: {e}"),
            });
            return;
        }
    };
    let r = restore_test::run(
        job.paths,
        job.id,
        job.profile,
        &job.profile.restore_test,
        &repo,
        progress,
        report,
    );
    let text = restore_test::summary(&r);
    if r.result == restore_test::TestResult::Failed {
        webhooks::send(
            job.paths,
            job.id,
            job.profile,
            flux_core::webhook::HookEvent::RestoreTestFailed,
            format!("Restore test of “{}”: {text}", job.profile.name),
            (None, Some(r.snapshot.clone()), r.error.clone()),
        );
        report(Event::Warning {
            path: String::new(),
            message: text.clone(),
        });
        if job.profile.options.notify {
            desktop::notify(
                &format!("Restore test of “{}” failed", job.profile.name),
                &text,
                desktop::Urgency::Critical,
            );
        }
    } else {
        report(Event::Info { message: text });
    }
}

/// "Shut down when done": schedule the poweroff in a minute, and say so. A backup that
/// was stopped doesn't shut down (the caller checks).
fn power_off_soon(job: &BackupJob<'_>, report: &dyn Fn(Event)) -> bool {
    match desktop::schedule_poweroff() {
        Ok(()) => {
            report(Event::Info {
                message: "shutting down in a minute, as asked (`shutdown -c` cancels it)".into(),
            });
            desktop::notify(
                "Shutting down in a minute",
                &format!(
                    "The backup of \u{201c}{}\u{201d} finished. Cancel in Flux Backup, or run shutdown -c.",
                    job.profile.name
                ),
                desktop::Urgency::Normal,
            );
            true
        }
        Err(e) => {
            report(Event::Warning {
                path: String::new(),
                message: format!("couldn't shut down: {e}"),
            });
            false
        }
    }
}

/// With "one backup at a time" on, wait until no other backup of this scope runs. The
/// job is already visible (and can be stopped) while it waits. `Err` if stopped meanwhile.
fn wait_for_turn(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: &dyn Fn(Event),
) -> Result<Option<jobs::Turn>, ()> {
    if job.dry_run || !prefs::Settings::load(job.paths).one_at_a_time {
        return Ok(None);
    }
    let mut told = false;
    loop {
        match jobs::try_turn(job.paths) {
            Ok(Some(t)) => {
                if told {
                    progress.set_phase(flux_core::progress::Phase::Preparing);
                    report(Event::Info {
                        message: "the other backup finished; starting".into(),
                    });
                }
                return Ok(Some(t));
            }
            Ok(None) if !told => {
                told = true;
                progress.set_phase(flux_core::progress::Phase::Waiting);
                let store = flux_core::ProfileStore::new(job.paths);
                let others: Vec<String> = jobs::running(job.paths)
                    .into_iter()
                    .filter(|j| j.profile != job.id.as_str())
                    .map(|j| {
                        flux_core::ProfileId::new(&j.profile)
                            .ok()
                            .and_then(|id| store.load(&id).ok())
                            .map_or(j.profile.clone(), |p| format!("\u{201c}{}\u{201d}", p.name))
                    })
                    .collect();
                report(Event::Info {
                    message: if others.is_empty() {
                        "waiting for another backup to finish (one backup at a time)".into()
                    } else {
                        format!(
                            "waiting for {} to finish (one backup at a time)",
                            others.join(" and ")
                        )
                    },
                });
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("cannot take the turn lock, not waiting: {e}");
                return Ok(None);
            }
        }
        if progress.is_cancelled() {
            return Err(());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn status_of(outcome: &Result<BackupOutcome, RunError>) -> RunStatus {
    match outcome {
        Ok(BackupOutcome::Created { snapshot, .. }) => match snapshot.status {
            SnapshotStatus::Complete => RunStatus::Completed,
            SnapshotStatus::CompleteWithErrors => RunStatus::CompletedWithErrors,
        },
        Ok(BackupOutcome::Unchanged { .. }) => RunStatus::Unchanged,
        Ok(BackupOutcome::DryRun { .. }) => RunStatus::DryRun,
        Err(RunError::Skipped(_) | RunError::Busy(_) | RunError::Held(_)) => RunStatus::Skipped,
        Err(e) if e.is_cancelled() => RunStatus::Cancelled,
        Err(_) => RunStatus::Failed,
    }
}

/// Why a scheduled run should not start at all (not due, on battery).
fn quick_skip(job: &BackupJob<'_>, now: Timestamp) -> Option<String> {
    let p = job.profile;
    if job.trigger != Trigger::Scheduled {
        return None;
    }
    if let Some(pause) = prefs::pause(job.paths, now) {
        return Some(match pause.until {
            Some(t) => format!(
                "scheduled backups are paused until {}",
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
            ),
            None => "scheduled backups are paused".into(),
        });
    }
    if p.schedule.skip_on_metered && matches!(p.target, Target::Ssh { .. }) && power::metered() {
        return Some("on a metered connection".into());
    }
    if matches!(p.target, Target::Ssh { .. })
        && (!p.network.wifi_only.is_empty() || !p.network.wifi_never.is_empty())
        && let Some(why) = p
            .network
            .wifi_refuses(flux_core::network::wifi_ssid().as_deref())
    {
        return Some(why);
    }
    if job.only_if_due
        && p.schedule.trigger == flux_core::profile::Trigger::Interval
        && let Some(every) = p.schedule.every
        && let Some(last) = runlog::recent_runs(job.paths, job.id, 200)
            .into_iter()
            .find(|r| {
                matches!(
                    r.status,
                    RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Unchanged
                )
            })
    {
        let due = last
            .finished
            .checked_add(every.as_duration())
            .unwrap_or(now);
        if now < due {
            let local = due
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M");
            return Some(format!("not due until {local}"));
        }
    }
    if p.schedule.skip_on_battery && power::on_battery() {
        return Some("running on battery".into());
    }
    if job.only_if_due && p.schedule.trigger == flux_core::profile::Trigger::Calendar {
        return calendar_not_due(job, now);
    }
    None
}

/// For cron (`--if-due` on a calendar schedule): has one of its times come since the last
/// check? Like systemd's `Persistent=`, a time missed while the computer was off runs once,
/// late, when catch-up is on; when it's off, a run more than [`cron::ON_TIME`] late is
/// skipped. A run is due at most once per time, whatever its outcome.
fn calendar_not_due(job: &BackupJob<'_>, now: Timestamp) -> Option<String> {
    let s = &job.profile.schedule;
    let expr = s.on_calendar.as_deref().unwrap_or("daily");
    let cal = match calendar::Calendar::parse(expr) {
        Ok(c) => c,
        Err(e) => return Some(format!("the schedule {expr:?} can't be read: {e}")),
    };
    let tz = jiff::tz::TimeZone::system();
    let local = |t: Timestamp| {
        t.to_zoned(tz.clone())
            .strftime("%Y-%m-%d %H:%M")
            .to_string()
    };
    let Some(last) = cron::last_check(job.paths, job.id) else {
        // A schedule starts when it's set up; earlier times weren't missed.
        cron::set_last_check(job.paths, job.id, now);
        return Some("the schedule starts now".into());
    };
    match calendar_due(&cal, last, now, s.catch_up, &tz) {
        Due::Now => {
            cron::set_last_check(job.paths, job.id, now);
            None
        }
        Due::Missed(due) => {
            cron::set_last_check(job.paths, job.id, now);
            Some(format!(
                "missed the run at {} (catching up is off)",
                local(due)
            ))
        }
        Due::NotUntil(due) => Some(format!("not due until {}", local(due))),
        Due::Never => Some(format!("the schedule {expr:?} never runs again")),
    }
}

/// What a calendar schedule asks for at `now`, last looked at at `last`.
#[derive(Debug, PartialEq, Eq)]
enum Due {
    Now,
    /// A time passed more than [`cron::ON_TIME`] ago, none is due now, and catching up is
    /// off.
    Missed(Timestamp),
    NotUntil(Timestamp),
    Never,
}

fn calendar_due(
    cal: &calendar::Calendar,
    last: Timestamp,
    now: Timestamp,
    catch_up: bool,
    tz: &jiff::tz::TimeZone,
) -> Due {
    match cal.next_after(last, tz) {
        Some(due) if due <= now => {
            let late = now.duration_since(due).unsigned_abs();
            if catch_up || late <= cron::ON_TIME {
                return Due::Now;
            }
            // The oldest time since the last look was missed. That says nothing about the
            // newest: after a day or more off, today's run may be due this very minute.
            let on_time_from = now
                .checked_sub(cron::ON_TIME)
                .ok()
                .and_then(|t| t.checked_sub(std::time::Duration::from_secs(1)).ok())
                .map_or(last, |t| t.max(last));
            match cal.next_after(on_time_from, tz) {
                Some(recent) if recent <= now => Due::Now,
                _ => Due::Missed(due),
            }
        }
        Some(due) => Due::NotUntil(due),
        None => Due::Never,
    }
}

fn unrecorded(
    job: &BackupJob<'_>,
    started: Timestamp,
    status: RunStatus,
    outcome: Result<BackupOutcome, RunError>,
) -> RunResult {
    let record = RunRecord {
        run_id: String::new(),
        profile: job.id.to_string(),
        trigger: job.trigger,
        started,
        finished: Timestamp::now(),
        status,
        snapshot: None,
        pruned: Vec::new(),
        error: outcome.as_ref().err().map(|e| e.to_string()),
        stats: None,
        warnings: 0,
        hooks: Vec::new(),
        free_after: None,
        sources_missing: Vec::new(),
        destinations: Vec::new(),
        unusual: None,
        sentry_seen: None,
        performance: None,
    };
    RunResult { record, outcome }
}

fn finish_run(
    job: &BackupJob<'_>,
    started: Timestamp,
    run_id: &str,
    progress: &Progress,
    outcome: &Result<BackupOutcome, RunError>,
    mut hooks_run: Vec<HookRun>,
) -> RunRecord {
    let p = job.profile;
    let status = status_of(outcome);
    if p.options.notify && !job.dry_run {
        match (outcome, status) {
            (Err(e), RunStatus::Failed) => desktop::notify(
                &format!("Backup of “{}” failed", p.name),
                &e.to_string(),
                desktop::Urgency::Critical,
            ),
            (Ok(BackupOutcome::Created { snapshot, .. }), RunStatus::CompletedWithErrors) => {
                desktop::notify(
                    &format!("Backup of “{}” finished with problems", p.name),
                    &format!(
                        "{} items could not be backed up. Run `flux info {} {}` for details.",
                        snapshot.errors_total, job.id, snapshot.id
                    ),
                    desktop::Urgency::Normal,
                )
            }
            (Ok(BackupOutcome::Created { snapshot, .. }), RunStatus::Completed)
                if job.trigger == Trigger::Scheduled
                    && prefs::Settings::load(job.paths).notify_success =>
            {
                desktop::notify(
                    &format!("Backed up “{}”", p.name),
                    &format!(
                        "{} new or changed files, {} copied.",
                        snapshot.stats.new + snapshot.stats.changed,
                        flux_core::ByteSize(snapshot.stats.copied_bytes).display_approx()
                    ),
                    desktop::Urgency::Normal,
                )
            }
            _ => {}
        }
    }
    let (snapshot, pruned, stats) = match outcome {
        Ok(BackupOutcome::Created { snapshot, pruned }) => (
            Some(snapshot.id.clone()),
            pruned.clone(),
            Some(snapshot.stats.clone()),
        ),
        Ok(BackupOutcome::Unchanged { previous, pruned }) => {
            (Some(previous.clone()), pruned.clone(), None)
        }
        _ => (None, Vec::new(), None),
    };
    for h in &mut hooks_run {
        h.output.truncate(h.output.floor_char_boundary(4096));
    }
    RunRecord {
        run_id: run_id.to_owned(),
        profile: job.id.to_string(),
        trigger: job.trigger,
        started,
        finished: Timestamp::now(),
        status,
        snapshot,
        pruned,
        error: outcome.as_ref().err().map(|e| e.to_string()),
        stats,
        warnings: progress.warnings.load(std::sync::atomic::Ordering::Relaxed),
        hooks: hooks_run,
        free_after: free_space(&p.target),
        sources_missing: Vec::new(),
        destinations: Vec::new(),
        unusual: None,
        sentry_seen: None,
        performance: {
            // The phase the engine was last in ends here.
            if progress.phase() != flux_core::progress::Phase::Done {
                progress.set_phase(flux_core::progress::Phase::Done);
            }
            runlog::Performance::of(progress)
        },
    }
}

/// Free space on a local target's filesystem.
pub fn free_space(target: &flux_core::profile::Target) -> Option<u64> {
    match target {
        flux_core::profile::Target::Local { path, .. } => rustix::fs::statvfs(path)
            .ok()
            .map(|v| v.f_bavail.saturating_mul(v.f_frsize)),
        flux_core::profile::Target::Ssh { .. } => None,
    }
}

/// Size of a local target's filesystem.
pub fn disk_size(target: &flux_core::profile::Target) -> Option<u64> {
    match target {
        flux_core::profile::Target::Local { path, .. } => rustix::fs::statvfs(path)
            .ok()
            .map(|v| v.f_blocks.saturating_mul(v.f_frsize)),
        flux_core::profile::Target::Ssh { .. } => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
    hooks_run: &mut Vec<HookRun>,
    sources_missing: &mut Vec<std::path::PathBuf>,
    destinations: &mut Vec<runlog::DestinationRun>,
    unusual: &mut Option<unusual::UnusualReport>,
    sentry_seen: &mut Option<sentry::Run>,
) -> Result<BackupOutcome, RunError> {
    // Sources whose drive isn't there: wait, fail, or back up the rest (see
    // `flux_core::source_check`). The engines see only the sources that are there.
    // Sources on a drive of their own that has no record yet (a profile from before 1.0,
    // or written by hand): recorded now, while the drive is there.
    if !job.dry_run {
        let mut p = job.profile.clone();
        if flux_core::source_check::record_volumes(&mut p) {
            let store = flux_core::ProfileStore::new(job.paths);
            if store.save(job.id, &p).is_ok() {
                for v in p
                    .sources
                    .volumes
                    .iter()
                    .filter(|v| !job.profile.sources.volumes.contains(v))
                {
                    report(Event::Info {
                        message: format!(
                            "{} is on a drive of its own (filesystem UUID {}): Flux will recognise when it isn't connected",
                            v.path.display(),
                            v.fs_uuid
                        ),
                    });
                }
            }
        }
    }
    // The profile as saved: its copy is kept with the backups (the engines see it reduced).
    let saved = job.profile;
    let (mut reduced, missing, carries) = sources_ready(job, progress, report)?;
    // With unusual-change detection on, the snapshot before the newest survives this
    // run's pruning: if the new one shows unusual changes, it's the one to keep.
    if reduced.unusual_changes != flux_core::profile::Sensitivity::Off {
        reduced.retention.keep_last = reduced.retention.keep_last.max(2);
    }
    let keep = if carries { missing.clone() } else { Vec::new() };
    let all_missing = missing.clone();
    // Sources left out of this snapshot altogether (their drive is away, and this kind
    // of backup doesn't carry their files over): not deletions, to Flux Sentry.
    let left_out = if carries { Vec::new() } else { missing.clone() };
    *sources_missing = missing;
    // After a backup with problems, keep everything until one runs cleanly (a bad run
    // mustn't push good snapshots out).
    let hold = job.prune
        && !job.dry_run
        && job.profile.retention.after_problems == flux_core::profile::AfterProblems::Hold
        && match last_problem(job) {
            Some(why) => {
                report(Event::Info {
                    message: format!(
                        "keeping every snapshot this time: the last backup {why} (old snapshots are removed again after a clean backup)"
                    ),
                });
                true
            }
            None => false,
        };
    let job = &BackupJob {
        paths: job.paths,
        id: job.id,
        profile: &reduced,
        name: job.name.clone(),
        dry_run: job.dry_run,
        trigger: job.trigger,
        prune: job.prune && !hold,
        only_if_due: job.only_if_due,
        ask: job.ask,
        shutdown_after: job.shutdown_after,
    };
    let keep = &keep;
    let p = job.profile;
    if job.trigger == Trigger::Scheduled {
        desktop::lower_priority(&p.options);
    }
    let _inhibit = (!job.dry_run)
        .then(|| desktop::inhibit_sleep(&format!("Backing up {}", p.name)))
        .flatten();

    let env = hook_env(job);
    let timeout = p
        .hooks
        .timeout
        .map_or(hooks::DEFAULT_TIMEOUT, |t| t.as_duration());
    let run_hook = |event: HookEvent,
                    cmds: &[String],
                    env: &BTreeMap<String, String>,
                    sink: &mut Vec<HookRun>| {
        if cmds.is_empty() || job.dry_run {
            return Ok(());
        }
        let (runs, r) = hooks::run_hooks(event, cmds, env, timeout);
        for run in &runs {
            report(Event::Info {
                message: format!(
                    "{} hook `{}` {}:\n{}",
                    event.as_str(),
                    run.command,
                    if run.success { "ok" } else { "failed" },
                    run.output.trim_end()
                ),
            });
        }
        sink.extend(runs);
        r
    };

    // Flux Sentry looks at the files before anything is written: a backup of files that
    // show the marks of an attack isn't made at all. (After the "before" hook, which may
    // be what makes the backups reachable.)
    let mut looked = false;
    let result = run_hook(HookEvent::PreBackup, &p.hooks.pre_backup, &env, hooks_run)
        .map_err(RunError::from)
        .and_then(|()| {
            let mut scanned = None;
            if !job.dry_run {
                // Tripwires where the profile asks for them and has none yet; none
                // where it doesn't (any more). Before the scan, so the snapshot holds
                // them. (A changed or removed one is never set again here: that is for
                // the look, just below, to find.)
                if p.tripwires && p.unusual_changes != flux_core::profile::Sensitivity::Off {
                    let planted = tripwire::plant(job.paths, job.id, p);
                    if planted > 0 {
                        report(Event::Info {
                            message: format!(
                                "Flux Sentry planted {planted} tripwire files ({}) in the folders this backs up",
                                tripwire::NAME
                            ),
                        });
                    }
                } else if !tripwire::load(job.paths, job.id).is_empty() {
                    // (Those on a drive that's away are removed when it's back.)
                    let removed = tripwire::remove(job.paths, job.id);
                    if removed > 0 {
                        report(Event::Info {
                            message: format!("Flux Sentry removed its {removed} tripwire files"),
                        });
                    }
                }
                let t = std::time::Instant::now();
                let seen = look_before(job, keep, &left_out, progress, report);
                progress
                    .timings
                    .record_phase("Looking for unusual changes", t.elapsed().as_secs_f64());
                match seen {
                    Looked::Unusual(u) => {
                        let why = format!(
                            "not backed up: Flux Sentry found unusual changes in the files ({}). The backups are as they were, and on hold until you've looked",
                            u.signals.join("; ")
                        );
                        *unusual = Some(*u);
                        return Err(RunError::Held(why));
                    }
                    Looked::Fine(seen, scan) => {
                        looked = true;
                        *sentry_seen = seen;
                        scanned = scan;
                    }
                    Looked::CouldNot(scan) => scanned = scan,
                }
            }
            engine_backup(job, saved, keep, scanned, progress, report)
        });

    // Unusual changes, judged against this backup's history: before anything goes to the
    // other places.
    // (Only where the look before the backup couldn't be had: backups written by rsync or
    // Borg, and a backup's first runs.)
    if let Ok(BackupOutcome::Created { snapshot, .. }) = &result
        && !job.dry_run
        && !looked
    {
        let t = std::time::Instant::now();
        *unusual = check_unusual(job, &snapshot.id, &left_out, sentry_seen, report);
        progress
            .timings
            .record_phase("Looking for unusual changes", t.elapsed().as_secs_f64());
    }
    // More destinations, each backed up from the sources in turn (independent copies: a
    // fault in one never reaches another). Not after a failed "before" hook or a stop.
    let hook_failed = matches!(result, Err(RunError::Hook(_)));
    if unusual.is_some() && !job.profile.destinations.is_empty() {
        // What looks like an attack stays out of the other places: their copies are as
        // they were before it, the ones kept elsewhere above all.
        report(Event::Info {
            message: "the other places weren't backed up to: they keep their copies from before the unusual changes".into(),
        });
        for d in &job.profile.destinations {
            destinations.push(destination_run(
                &d.name,
                &Err(RunError::Skipped(
                    "Flux Sentry found unusual changes: this place keeps its copy from before them"
                        .into(),
                )),
            ));
        }
    } else if !job.profile.destinations.is_empty() && !hook_failed {
        // The main backup failed before Flux Sentry had its look (its drive is away, its
        // server unreachable): nothing has compared the files with a snapshot, and the
        // other places must not take what looks like an attack unlooked-at. Each is
        // looked at against its own newest snapshot: before it's written where that can
        // be had (Flux's own formats), right after where it can't (rsync, Borg).
        let mut unlooked = result.is_err()
            && !looked
            && !job.dry_run
            && job.profile.unusual_changes != flux_core::profile::Sensitivity::Off;
        for d in &job.profile.destinations {
            if progress.is_cancelled() {
                break;
            }
            if unusual.is_some() {
                // Found while looking for an earlier place: the rest keep their copies.
                destinations.push(destination_run(
                    &d.name,
                    &Err(RunError::Skipped(
                        "Flux Sentry found unusual changes: this place keeps its copy from before them"
                            .into(),
                    )),
                ));
                continue;
            }
            let (Ok(did), Some(dp)) = (
                job.id.with_destination(&d.name),
                job.profile.for_destination(&d.name),
            ) else {
                continue;
            };
            // The Wi-Fi rules apply to each place on a server, for scheduled backups.
            if job.trigger == Trigger::Scheduled
                && matches!(dp.target, Target::Ssh { .. })
                && (!dp.network.wifi_only.is_empty() || !dp.network.wifi_never.is_empty())
                && let Some(why) = dp
                    .network
                    .wifi_refuses(flux_core::network::wifi_ssid().as_deref())
            {
                let r = Err(RunError::Skipped(why));
                if let Err(RunError::Skipped(why)) = &r {
                    report(Event::Info {
                        message: format!("{}: skipped, {why}", d.label()),
                    });
                }
                destinations.push(destination_run(&d.name, &r));
                continue;
            }
            progress.set_phase(flux_core::progress::Phase::Preparing);
            // Each place counts from nothing: what the main backup did isn't part of
            // this one's progress (it showed as more than all of it).
            progress
                .entries_done
                .store(0, std::sync::atomic::Ordering::Relaxed);
            progress
                .bytes_done
                .store(0, std::sync::atomic::Ordering::Relaxed);
            report(Event::Info {
                message: format!("backing up to {}", d.label()),
            });
            let djob = BackupJob {
                paths: job.paths,
                id: &did,
                profile: &dp,
                name: job.name.clone(),
                dry_run: job.dry_run,
                trigger: job.trigger,
                prune: job.prune,
                only_if_due: false,
                ask: job.ask,
                shutdown_after: false,
            };
            let r = if d.mode == flux_core::profile::CopyMode::Replicate {
                replicate_to(job, &djob, &result, progress, report)
            } else {
                let dkeep = if engine_keeps(&dp) {
                    all_missing.clone()
                } else {
                    Vec::new()
                };
                let dsaved = saved.for_destination(&d.name).unwrap_or_else(|| dp.clone());
                let mut look_after = unlooked;
                if unlooked
                    && matches!(dp.engine, Engine::Mirror | Engine::Vault)
                    && let Ok(drepo) = repo::AnyRepo::open(djob.paths, djob.id, &dp, djob.ask)
                    && let Some(dprev) = drepo.snapshots().ok().and_then(|mut s| s.pop())
                {
                    // The main backup's history, this place's files and folders.
                    let look = BackupJob {
                        paths: job.paths,
                        id: job.id,
                        profile: &dp,
                        name: None,
                        dry_run: false,
                        trigger: job.trigger,
                        prune: false,
                        only_if_due: false,
                        ask: job.ask,
                        shutdown_after: false,
                    };
                    // (`dkeep` is every missing source when this place carries them
                    // over, none when it leaves them out.)
                    let dleft: &[std::path::PathBuf] =
                        if dkeep.is_empty() { &all_missing } else { &[] };
                    let compared = scan_for_look(&look)
                        .and_then(|s| compare_with(&look, &drepo, &dprev, &s, &dkeep, dleft));
                    match compared {
                        Some((_, Some(mut u))) => {
                            u.stopped = true;
                            u.snapshot = format!("before:{}", Timestamp::now());
                            u.before = Some(dprev.id.clone());
                            report(Event::Warning {
                                path: String::new(),
                                message: format!(
                                    "Flux Sentry stopped the backup to {} before anything was written",
                                    d.label()
                                ),
                            });
                            (u.before_pinned, u.pinned_by_flux) =
                                raise_alarm(job, Some((&drepo, &dprev)), &u, report);
                            *unusual = Some(u);
                            destinations.push(destination_run(
                                &d.name,
                                &Err(RunError::Skipped(
                                    "Flux Sentry found unusual changes: this place keeps its copy from before them"
                                        .into(),
                                )),
                            ));
                            continue;
                        }
                        // Nothing unusual in the files as they are: that holds for
                        // every place.
                        Some((_, None)) => {
                            unlooked = false;
                            look_after = false;
                        }
                        None => {}
                    }
                }
                let r = engine_backup(&djob, &dsaved, &dkeep, None, progress, report);
                if look_after && let Ok(BackupOutcome::Created { snapshot, .. }) = &r {
                    let dleft: &[std::path::PathBuf] =
                        if dkeep.is_empty() { &all_missing } else { &[] };
                    *unusual = check_unusual(&djob, &snapshot.id, dleft, &mut None, report);
                }
                r
            };
            if let Err(RunError::Skipped(why)) = &r {
                report(Event::Info {
                    message: format!("{}: {why}", d.label()),
                });
            } else if let Err(e) = &r
                && !e.is_cancelled()
            {
                report(Event::Warning {
                    path: String::new(),
                    message: format!("the backup to {} failed: {e}", d.label()),
                });
                if p.options.notify && !job.dry_run {
                    desktop::notify(
                        &format!("Backup of “{}” to {} failed", p.name, d.label()),
                        &e.to_string(),
                        desktop::Urgency::Critical,
                    );
                }
            }
            destinations.push(destination_run(&d.name, &r));
        }
    }

    match &result {
        Ok(outcome) => {
            let mut env = env.clone();
            let status = match outcome {
                BackupOutcome::Created { snapshot, .. } => {
                    env.insert("FLUX_SNAPSHOT_ID".into(), snapshot.id.clone());
                    if p.engine == Engine::Rsync {
                        env.insert(
                            "FLUX_SNAPSHOT_PATH".into(),
                            format!(
                                "{}/{}/{}",
                                flux_compat::rsync::repo_location(job.id, p),
                                flux_compat::rsync::SNAPSHOTS,
                                snapshot.id
                            ),
                        );
                    } else if let Ok(repo) = flux_mirror::open_repo(job.id, p) {
                        env.insert(
                            "FLUX_SNAPSHOT_PATH".into(),
                            repo.snapshot_dir(&snapshot.id).display().to_string(),
                        );
                    }
                    match snapshot.status {
                        SnapshotStatus::Complete => "complete",
                        SnapshotStatus::CompleteWithErrors => "complete_with_errors",
                    }
                }
                BackupOutcome::Unchanged { previous, .. } => {
                    env.insert("FLUX_SNAPSHOT_ID".into(), previous.clone());
                    "unchanged"
                }
                BackupOutcome::DryRun { .. } => "dry_run",
            };
            env.insert("FLUX_STATUS".into(), status.into());
            if let Err(e) = run_hook(HookEvent::PostBackup, &p.hooks.post_backup, &env, hooks_run) {
                report(Event::Warning {
                    path: String::new(),
                    message: e.to_string(),
                });
            }
        }
        Err(e) if e.is_cancelled() => {}
        Err(err) => {
            let mut env = env.clone();
            env.insert("FLUX_STATUS".into(), "failed".into());
            env.insert("FLUX_ERROR".into(), err.to_string());
            if let Err(e) = run_hook(HookEvent::OnError, &p.hooks.on_error, &env, hooks_run) {
                report(Event::Warning {
                    path: String::new(),
                    message: e.to_string(),
                });
            }
        }
    }
    result
}

fn borg_backup(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let p = job.profile;
    let passphrase = if p.password_source().is_some() && !job.dry_run {
        let exists = flux_compat::borg::repository_exists(job.paths, job.id, p, job.ask.is_none())?;
        Some(
            password::obtain(
                job.id,
                p,
                if exists {
                    password::Purpose::Open
                } else {
                    password::Purpose::Create
                },
                job.ask,
            )
            .map_err(repo::RepoError::from)?,
        )
    } else if p.password_source().is_some() {
        Some(
            password::obtain(job.id, p, password::Purpose::Open, job.ask)
                .map_err(repo::RepoError::from)?,
        )
    } else {
        None
    };
    Ok(flux_compat::borg::backup(
        &flux_compat::borg::BackupRequest {
            paths: job.paths,
            id: job.id,
            profile: p,
            name: job.name.clone(),
            dry_run: job.dry_run,
            prune: job.prune,
            batch: job.ask.is_none(),
            passphrase,
        },
        progress,
        report,
    )?)
}

/// Back up `job` with its engine. `keep`: sources whose drive is missing, kept from the
/// previous snapshot (Mirror on a local target and Vault; empty for the others).
/// `scanned`: the scan of the sources, when it was made already (for Flux Sentry's look).
fn engine_backup(
    job: &BackupJob<'_>,
    saved: &Profile,
    keep: &[std::path::PathBuf],
    scanned: Option<flux_fs::scan::ScanResult>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let keep = &keep.to_vec();
    // The upload limit in force now (a limit by time of day); Vault also follows it as
    // the backup runs.
    let timed = repo::with_bandwidth_now(job.profile);
    let job = &BackupJob {
        paths: job.paths,
        id: job.id,
        profile: &timed,
        name: job.name.clone(),
        dry_run: job.dry_run,
        trigger: job.trigger,
        prune: job.prune,
        only_if_due: job.only_if_due,
        ask: job.ask,
        shutdown_after: job.shutdown_after,
    };
    let p = job.profile;
    let out = match p.engine {
        Engine::Mirror if matches!(p.target, Target::Ssh { .. }) => {
            let pool = repo::sftp_pool(job.paths, p, job.ask.is_none()).expect("ssh target");
            flux_mirror::remote::backup_scanned(
                pool,
                &flux_mirror::remote::RemoteBackup {
                    id: job.id,
                    profile: p,
                    name: job.name.clone(),
                    dry_run: job.dry_run,
                    prune: job.prune,
                    cache_dir: &job.paths.cache_dir,
                },
                scanned,
                progress,
                report,
            )
            .map_err(RunError::from)
        }
        Engine::Mirror => flux_mirror::backup_scanned(
            &BackupRequest {
                id: job.id,
                profile: p,
                name: job.name.clone(),
                dry_run: job.dry_run,
                prune: job.prune,
                keep: keep.clone(),
            },
            scanned,
            progress,
            report,
        )
        .map_err(RunError::from),
        Engine::Vault => vault_backup(job, saved, keep, scanned, progress, report),
        Engine::Borg => borg_backup(job, progress, report),
        Engine::Rsync => flux_compat::rsync::backup(
            &flux_compat::rsync::BackupRequest {
                paths: job.paths,
                id: job.id,
                profile: p,
                name: job.name.clone(),
                dry_run: job.dry_run,
                prune: job.prune,
                batch: job.ask.is_none(),
            },
            progress,
            report,
        )
        .map_err(RunError::from),
    };
    // The settings, kept with the backups for recovering them on another computer (a
    // Vault keeps them while it's open, in `vault_backup`).
    if !job.dry_run
        && p.engine != Engine::Vault
        && matches!(
            out,
            Ok(BackupOutcome::Created { .. } | BackupOutcome::Unchanged { .. })
        )
        && let Err(e) = recover::save_copy(job.paths, job.id, saved, job.ask.is_none())
    {
        report(Event::Warning {
            path: String::new(),
            message: format!("couldn't keep this backup's settings with it: {e}"),
        });
    }
    out
}

/// Engines that keep a missing source's files from the previous snapshot.
/// Copy the main destination's backup (`main`, just made by `outcome`) to the replica
/// `copy`: the same snapshots, the sources not read again. The outcome is the main one's
/// (the snapshot is the same).
fn replicate_to(
    main: &BackupJob<'_>,
    copy: &BackupJob<'_>,
    outcome: &Result<BackupOutcome, RunError>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let out = match outcome {
        Ok(o) => o.clone(),
        Err(_) => {
            return Err(RunError::Skipped(
                "not copied: the main backup didn't finish, so there was nothing new to copy"
                    .into(),
            ));
        }
    };
    if main.dry_run {
        return Ok(out);
    }
    let (p, cp) = (main.profile, copy.profile);
    if let Target::Local { path, fs_uuid } = &cp.target {
        flux_core::target_check::check_local_target(path, fs_uuid.as_deref())
            .map_err(repo::RepoError::from)?;
    }
    let label = cp.name.rsplit(" → ").next().unwrap_or(&cp.name).to_owned();
    match p.engine {
        Engine::Vault => {
            let src = repo::vault_backend(p, repo::sftp_pool(main.paths, p, main.ask.is_none()));
            let dst = repo::vault_backend(cp, repo::sftp_pool(copy.paths, cp, copy.ask.is_none()));
            let r = flux_vault::replicate::replicate(src.as_ref(), dst.as_ref(), progress)?;
            report(Event::Info {
                message: format!(
                    "copied the Vault to {label}: {} files, {}",
                    r.copied_files,
                    flux_core::ByteSize(r.copied_bytes).display_approx()
                ),
            });
        }
        Engine::Mirror | Engine::Rsync => {
            let root = |id: &ProfileId,
                        prof: &flux_core::Profile|
             -> Result<std::path::PathBuf, RunError> {
                Ok(if prof.engine == Engine::Mirror {
                    flux_mirror::repo_root(id, prof)?.0
                } else {
                    std::path::PathBuf::from(flux_compat::rsync::repo_location(id, prof))
                })
            };
            let (src, dst) = (root(main.id, p)?, root(copy.id, cp)?);
            let r = flux_mirror::replicate::replicate(
                &src,
                &dst,
                copy.id.as_str(),
                flux_fs::scan::xattr_policy(p),
                progress,
                report,
            )?;
            if p.engine == Engine::Rsync
                && let Some(newest) = r.copied.last()
            {
                // The replica's own recovery guide, with its own paths.
                let version = flux_compat::tools::rsync_status()
                    .version
                    .unwrap_or_else(|| "3.x".into());
                let kit = flux_compat::kit::rsync_kit(copy.id, cp, &version, Some(newest));
                let _ = std::fs::write(dst.join(flux_compat::kit::RECOVERY_FILE), kit.recovery);
                let _ = std::fs::write(dst.join(flux_compat::kit::SCRIPT_FILE), kit.script);
            }
            report(Event::Info {
                message: format!(
                    "copied {} snapshots to {label} ({} files copied, {} linked){}",
                    r.copied.len(),
                    r.stats.copied_files,
                    r.stats.linked_files,
                    if r.removed.is_empty() {
                        String::new()
                    } else {
                        format!("; removed {} gone from the main backup", r.removed.len())
                    }
                ),
            });
        }
        Engine::Borg => {
            return Err(RunError::Unsupported(
                "Borg repositories can't be copied; back up to this place from the sources".into(),
            ));
        }
    }
    Ok(out)
}

/// A destination's part of a run, from its outcome.
fn destination_run(name: &str, r: &Result<BackupOutcome, RunError>) -> runlog::DestinationRun {
    let (snapshot, stats) = match r {
        Ok(BackupOutcome::Created { snapshot, .. }) => {
            (Some(snapshot.id.clone()), Some(snapshot.stats.clone()))
        }
        Ok(BackupOutcome::Unchanged { previous, .. }) => (Some(previous.clone()), None),
        _ => (None, None),
    };
    runlog::DestinationRun {
        name: name.to_owned(),
        status: status_of(r),
        snapshot,
        error: r.as_ref().err().map(|e| e.to_string()),
        stats,
    }
}

fn engine_keeps(p: &flux_core::Profile) -> bool {
    p.engine == Engine::Vault
        || (p.engine == Engine::Mirror && matches!(p.target, Target::Local { .. }))
}

fn vault_backup(
    job: &BackupJob<'_>,
    saved: &Profile,
    keep: &[std::path::PathBuf],
    scanned: Option<flux_fs::scan::ScanResult>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let p = job.profile;
    let repo = repo::open_vault(job.paths, job.id, p, !job.dry_run, job.ask)?;
    let owner = repo::owner_of(job.id, p);
    let never = vault_never(p);
    let out = flux_vault::backup::backup_scanned(
        &repo,
        &flux_vault::backup::BackupRequest {
            owner: &owner,
            profile: p,
            name: job.name.clone(),
            dry_run: job.dry_run,
            never,
            cache_dir: Some(job.paths.cache_dir.join("vault")),
            keep: keep.to_vec(),
        },
        scanned,
        progress,
        report,
    )?;
    // The settings, kept with the backups for recovering them on another computer.
    if !job.dry_run
        && matches!(
            out,
            BackupOutcome::Created { .. } | BackupOutcome::Unchanged { .. }
        )
        && let Err(e) = recover::save_copy_in_vault(job.paths, job.id, saved, &repo, &owner)
    {
        report(Event::Warning {
            path: String::new(),
            message: format!("couldn't keep this backup's settings in the Vault: {e}"),
        });
    }
    // The program that reads Vaults without Flux, next to a Vault on a drive.
    if !job.dry_run
        && let Err(e) = recover::keep_tool(saved, recover::recovery_tool().as_deref())
    {
        report(Event::Warning {
            path: String::new(),
            message: format!("couldn't keep the recovery tool next to the Vault: {e}"),
        });
    }
    if !job.prune || job.dry_run {
        return Ok(out);
    }
    // Forget snapshots the policy no longer keeps. Their space is reclaimed by `flux prune`,
    // which needs an exclusive lock and rewrites packs; that's too heavy for every backup.
    let forgotten = {
        let _lock = repo.lock(false)?;
        flux_vault::gc::forget(&repo, &owner, &p.retention, false, report)?.1
    };
    Ok(match out {
        BackupOutcome::Created { snapshot, .. } => BackupOutcome::Created {
            snapshot,
            pruned: forgotten,
        },
        BackupOutcome::Unchanged { previous, .. } => BackupOutcome::Unchanged {
            previous,
            pruned: forgotten,
        },
        other => other,
    })
}

fn hook_env(job: &BackupJob<'_>) -> BTreeMap<String, String> {
    let p = job.profile;
    let target = match &p.target {
        Target::Local { path, .. } => path.display().to_string(),
        Target::Ssh {
            host,
            user,
            port,
            path,
            ..
        } => format!(
            "ssh://{}{host}{}/{}",
            user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
            port.map(|p| format!(":{p}")).unwrap_or_default(),
            path.display().to_string().trim_start_matches('/')
        ),
    };
    BTreeMap::from([
        ("FLUX_PROFILE".into(), job.id.to_string()),
        ("FLUX_PROFILE_NAME".into(), p.name.clone()),
        ("FLUX_ENGINE".into(), p.engine.to_string()),
        ("FLUX_TARGET".into(), target),
        (
            "FLUX_SOURCES".into(),
            p.sources
                .paths
                .iter()
                .map(|s| s.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "FLUX_TRIGGER".into(),
            match job.trigger {
                Trigger::Manual => "manual".into(),
                Trigger::Scheduled => "scheduled".into(),
            },
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With catching up off, a run missed days ago must not cost today's run, which is
    /// due this minute.
    #[test]
    fn a_missed_run_does_not_skip_the_one_due_now() {
        let tz = jiff::tz::TimeZone::UTC;
        let cal = calendar::Calendar::parse("*-*-* 07:00:00").unwrap();
        let at = |s: &str| -> Timestamp { s.parse().unwrap() };
        let now = at("2026-11-09T07:00:20Z");
        let due = |last: &str, catch_up| calendar_due(&cal, at(last), now, catch_up, &tz);
        // Checked yesterday after its run: today's is due.
        assert_eq!(due("2026-11-08T07:01:00Z", false), Due::Now);
        // Off since Friday: Saturday's and Sunday's were missed, Monday's is due now.
        assert_eq!(due("2026-11-06T07:01:00Z", false), Due::Now);
        assert_eq!(due("2026-11-06T07:01:00Z", true), Due::Now);
        // Already looked at this minute: tomorrow.
        assert_eq!(
            due("2026-11-09T07:00:05Z", false),
            Due::NotUntil(at("2026-11-10T07:00:00Z"))
        );
        // Switched on at noon: the 07:00 run was missed (it runs late only when catching up).
        let noon = at("2026-11-09T12:00:00Z");
        let last = at("2026-11-08T07:01:00Z");
        assert_eq!(
            calendar_due(&cal, last, noon, false, &tz),
            Due::Missed(at("2026-11-09T07:00:00Z"))
        );
        assert_eq!(calendar_due(&cal, last, noon, true, &tz), Due::Now);
    }
}
