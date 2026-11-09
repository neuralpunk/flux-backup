//! The job runner: everything around an engine run, shared by the CLI, the scheduler and
//! the GUI.
//!
//! ```text
//! pause, battery and metered checks (scheduled runs) → lower priority (scheduled runs)
//!   → inhibit suspend → pre_backup hooks → engine backup (+ retention)
//!   → post_backup / on_error hooks → upkeep when due (scheduled runs)
//!   → notification (failures; successes if asked) → run record + event log
//! ```

pub mod calendar;
pub mod cron;
pub mod desktop;
pub mod index;
pub mod jobs;
pub mod maintenance;
pub mod password;
pub mod prefs;
pub mod repo;
pub mod runlog;
pub mod schedule;
pub mod view;

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
        let turn = wait_for_turn(job, progress, &tee);
        let outcome = match &turn {
            Err(()) => Err(RunError::Engine(MirrorError::Cancelled)),
            Ok(_) => run_inner(job, progress, &tee, &mut hooks_run),
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
        let record = finish_run(job, started, &log.run_id, progress, &outcome, hooks_run);
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
    }
    drop(lock);
    RunResult { record, outcome }
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
    match cal.next_after(last, &tz) {
        Some(due) if due <= now => {
            cron::set_last_check(job.paths, job.id, now);
            let late = now.duration_since(due).unsigned_abs();
            if !s.catch_up && late > cron::ON_TIME {
                return Some(format!(
                    "missed the run at {} (catching up is off)",
                    local(due)
                ));
            }
            None
        }
        Some(due) => Some(format!("not due until {}", local(due))),
        None => Some(format!("the schedule {expr:?} never runs again")),
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
) -> Result<BackupOutcome, RunError> {
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
        .and_then(|()| match p.engine {
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
                },
                progress,
                report,
            )
            .map_err(RunError::from),
            Engine::Vault => vault_backup(job, progress, report),
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
        });

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

fn vault_backup(
    job: &BackupJob<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, RunError> {
    let p = job.profile;
    let repo = repo::open_vault(job.paths, job.id, p, !job.dry_run, job.ask)?;
    let owner = repo::owner_for(job.id);
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
        },
        progress,
        report,
    )?;
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
