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
pub mod schedule;
pub mod templates;
pub mod unusual;
pub mod view;
pub mod webhooks;

use std::collections::BTreeMap;

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
        let mut hooks_run: Vec<HookRun> = Vec::new();
        let mut sources_missing: Vec<std::path::PathBuf> = Vec::new();
        let mut destinations: Vec<runlog::DestinationRun> = Vec::new();
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
        // Unusual changes, judged against this backup's history.
        let unusual = match &outcome {
            Ok(BackupOutcome::Created { snapshot, .. }) if !job.dry_run => {
                let t = std::time::Instant::now();
                let found = check_unusual(job, &snapshot.id, &tee);
                progress
                    .timings
                    .record_phase("Looking for unusual changes", t.elapsed().as_secs_f64());
                found
            }
            _ => None,
        };
        let mut record = finish_run(job, started, &log.run_id, progress, &outcome, hooks_run);
        record.unusual = unusual;
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
        _ if last.unusual.is_some() && !unusual_dismissed(job.paths, job.id, &last) => {
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
fn check_unusual(
    job: &BackupJob<'_>,
    snapshot: &str,
    report: Reporter<'_>,
) -> Option<unusual::UnusualReport> {
    let sens = job.profile.unusual_changes;
    if sens == flux_core::profile::Sensitivity::Off {
        return None;
    }
    let repo = repo::AnyRepo::open(job.paths, job.id, job.profile, job.ask).ok()?;
    let snaps = repo.snapshots().ok()?;
    let i = snaps.iter().position(|s| s.id == snapshot)?;
    let prev = snaps[..i].last()?.clone();
    let now = snaps[i].clone();
    let entries = |s: &flux_fs::snapshot::SnapshotMeta| -> Option<Vec<flux_fs::entry::Entry>> {
        repo.entries(s).ok()?.collect::<Result<Vec<_>, _>>().ok()
    };
    let (before, after) = (entries(&prev)?, entries(&now)?);
    let history = runlog::recent_runs(job.paths, job.id, 20);
    let sample = |p: &std::path::Path| -> Option<Vec<u8>> {
        use std::io::Read;
        let mut buf = Vec::with_capacity(64 << 10);
        std::fs::File::open(p)
            .ok()?
            .take(64 << 10)
            .read_to_end(&mut buf)
            .ok()?;
        Some(buf)
    };
    let mut r = unusual::analyze(&before, &after, &history, sens, &sample)?;
    r.snapshot = now.id.clone();
    r.before = Some(prev.id.clone());
    let day = prev
        .started
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d %H:%M")
        .to_string();
    // Mark the new snapshot; keep the one before for sure.
    let mut tags = now.tags.clone();
    tags.push(UNUSUAL_TAG.into());
    let _ = repo.change(
        &now.id,
        &flux_fs::snapshot::SnapshotChange {
            tags: Some(tags),
            ..Default::default()
        },
    );
    if !prev.pinned {
        let _ = repo.change(
            &prev.id,
            &flux_fs::snapshot::SnapshotChange {
                pinned: Some(true),
                note: prev
                    .note
                    .is_none()
                    .then(|| Some(format!("{AUTO_PIN_NOTE} ({day})."))),
                ..Default::default()
            },
        );
    }
    for s in &r.signals {
        report(Event::Warning {
            path: String::new(),
            message: format!("unusual changes: {s}"),
        });
    }
    report(Event::Info {
        message: format!(
            "kept snapshot {} for sure (pinned): the last one before these changes",
            prev.id
        ),
    });
    if job.profile.options.notify {
        desktop::notify(
            &format!("Unusual changes in “{}”", job.profile.name),
            &format!(
                "{}. The snapshot from {day}, before them, is kept. If you didn't make these changes, don't delete anything and check your files.",
                r.signals.join("; ")
            ),
            desktop::Urgency::Critical,
        );
    }
    Some(r)
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

/// The person said the unusual changes in `snapshot` were theirs.
pub fn dismiss_unusual(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    snapshot: &str,
) -> std::io::Result<()> {
    let p = unusual_state(paths, id);
    std::fs::create_dir_all(p.parent().expect("state folder"))?;
    flux_core::store::write_atomic(&p, snapshot.as_bytes(), 0o600).map_err(std::io::Error::other)
}

/// Were the unusual changes of this run dismissed ("This was me")?
pub fn unusual_dismissed(
    paths: &flux_core::Paths,
    id: &flux_core::ProfileId,
    run: &RunRecord,
) -> bool {
    run.unusual.as_ref().is_some_and(|u| {
        std::fs::read_to_string(unusual_state(paths, id)).is_ok_and(|s| s.trim() == u.snapshot)
    })
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
        Err(RunError::Skipped(_) | RunError::Busy(_)) => RunStatus::Skipped,
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

fn run_inner(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
    hooks_run: &mut Vec<HookRun>,
    sources_missing: &mut Vec<std::path::PathBuf>,
    destinations: &mut Vec<runlog::DestinationRun>,
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

    let result = run_hook(HookEvent::PreBackup, &p.hooks.pre_backup, &env, hooks_run)
        .map_err(RunError::from)
        .and_then(|()| engine_backup(job, saved, keep, progress, report));

    // More destinations, each backed up from the sources in turn (independent copies: a
    // fault in one never reaches another). Not after a failed "before" hook or a stop.
    let hook_failed = matches!(result, Err(RunError::Hook(_)));
    if !job.profile.destinations.is_empty() && !hook_failed {
        for d in &job.profile.destinations {
            if progress.is_cancelled() {
                break;
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
                engine_backup(&djob, &dsaved, &dkeep, progress, report)
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
fn engine_backup(
    job: &BackupJob<'_>,
    saved: &Profile,
    keep: &[std::path::PathBuf],
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
            flux_mirror::remote::backup(
                pool,
                &flux_mirror::remote::RemoteBackup {
                    id: job.id,
                    profile: p,
                    name: job.name.clone(),
                    dry_run: job.dry_run,
                    prune: job.prune,
                    cache_dir: &job.paths.cache_dir,
                },
                progress,
                report,
            )
            .map_err(RunError::from)
        }
        Engine::Mirror => flux_mirror::backup(
            &BackupRequest {
                id: job.id,
                profile: p,
                name: job.name.clone(),
                dry_run: job.dry_run,
                prune: job.prune,
                keep: keep.clone(),
            },
            progress,
            report,
        )
        .map_err(RunError::from),
        Engine::Vault => vault_backup(job, saved, keep, progress, report),
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
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let p = job.profile;
    let repo = repo::open_vault(job.paths, job.id, p, !job.dry_run, job.ask)?;
    let owner = repo::owner_of(job.id, p);
    let never = match &p.target {
        Target::Local { .. } => {
            let root = repo::vault_root(p);
            vec![root.clone(), root.with_file_name("flux-backup")]
        }
        Target::Ssh { .. } => Vec::new(),
    };
    let out = flux_vault::backup::backup(
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
