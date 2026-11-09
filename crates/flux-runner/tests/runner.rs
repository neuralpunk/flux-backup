//! Runner behaviour: hooks, their environment, failure handling and run logs.

use std::fs;
use std::path::PathBuf;

use flux_core::profile::{Profile, Target};
use flux_core::progress::{Progress, silent};
use flux_core::{Paths, ProfileId, Scope};
use flux_runner::{BackupJob, RunError, RunStatus, Trigger, run_backup, runlog};

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    id: ProfileId,
    profile: Profile,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tgt")).unwrap();
    fs::write(root.join("src/f"), "data").unwrap();
    let mut profile = Profile::new(
        "Test",
        Target::Local {
            path: root.join("tgt"),
            fs_uuid: None,
        },
        vec![root.join("src")],
    );
    profile.options.notify = false; // never pop up notifications from tests
    Env {
        paths: Paths::under(root.join("home"), Scope::User),
        _tmp: tmp,
        root,
        id: ProfileId::new("t").unwrap(),
        profile,
    }
}

impl Env {
    fn run(&self) -> flux_runner::RunResult {
        run_backup(
            &BackupJob {
                paths: &self.paths,
                id: &self.id,
                profile: &self.profile,
                name: None,
                dry_run: false,
                trigger: Trigger::Manual,
                prune: true,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        )
    }
}

#[test]
fn hooks_get_environment_and_run_in_order() {
    let mut e = env();
    let out = e.root.join("hook.log");
    let o = out.display();
    e.profile.hooks.pre_backup = vec![format!(
        "echo \"pre $FLUX_EVENT $FLUX_PROFILE $FLUX_TRIGGER\" >> {o}"
    )];
    e.profile.hooks.post_backup = vec![format!(
        "echo \"post $FLUX_STATUS $FLUX_SNAPSHOT_ID\" >> {o}; test -d \"$FLUX_SNAPSHOT_PATH\" && echo has-path >> {o}"
    )];
    let r = e.run();
    let snap = r.record.snapshot.clone().unwrap();
    assert_eq!(r.record.status, RunStatus::Completed);
    let log = fs::read_to_string(&out).unwrap();
    assert_eq!(
        log,
        format!("pre pre_backup t manual\npost complete {snap}\nhas-path\n")
    );
    assert_eq!(r.record.hooks.len(), 2);

    // Recorded in the history, newest first.
    let r2 = e.run();
    assert_eq!(r2.record.status, RunStatus::Unchanged);
    let runs = runlog::recent_runs(&e.paths, &e.id, 10);
    assert_eq!(runs.len(), 2);
    assert_ne!(runs[0].run_id, runs[1].run_id, "run ids are unique");
    assert_eq!(runs[0].status, RunStatus::Unchanged);
    assert!(
        fs::read_to_string(&out)
            .unwrap()
            .ends_with(&format!("post unchanged {snap}\n"))
    );
}

#[test]
fn failing_pre_hook_aborts_and_runs_on_error() {
    let mut e = env();
    let out = e.root.join("err.log");
    e.profile.hooks.pre_backup = vec!["echo mounting...; exit 7".into()];
    e.profile.hooks.on_error = vec![format!(
        "echo \"$FLUX_STATUS: $FLUX_ERROR\" > {}",
        out.display()
    )];
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Failed);
    assert!(matches!(r.outcome, Err(RunError::Hook(_))));
    assert!(
        !e.root.join("tgt/flux-backup").exists(),
        "no backup after a failed pre hook"
    );
    let msg = fs::read_to_string(&out).unwrap();
    assert!(msg.starts_with("failed: pre_backup hook"), "{msg}");
    // The hook's output lands in the run's event log.
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.contains("mounting..."))));
}

#[test]
fn engine_failure_is_recorded() {
    let mut e = env();
    e.profile.target = Target::Local {
        path: e.root.join("missing-drive"),
        fs_uuid: None,
    };
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Failed);
    assert!(
        r.record
            .error
            .as_deref()
            .unwrap()
            .contains("does not exist")
    );
    assert_eq!(
        runlog::recent_runs(&e.paths, &e.id, 1)[0].status,
        RunStatus::Failed
    );
}

#[test]
fn dry_run_runs_no_hooks_and_writes_no_log() {
    let mut e = env();
    e.profile.hooks.pre_backup = vec!["exit 1".into()];
    let r = run_backup(
        &BackupJob {
            paths: &e.paths,
            id: &e.id,
            profile: &e.profile,
            name: None,
            dry_run: true,
            trigger: Trigger::Manual,
            prune: false,
            only_if_due: false,
            ask: None,
            shutdown_after: false,
        },
        &Progress::new(),
        &silent,
    );
    assert_eq!(r.record.status, RunStatus::DryRun);
    assert!(runlog::recent_runs(&e.paths, &e.id, 1).is_empty());
}

fn scheduled(e: &Env, only_if_due: bool) -> flux_runner::RunResult {
    run_backup(
        &BackupJob {
            paths: &e.paths,
            id: &e.id,
            profile: &e.profile,
            name: None,
            dry_run: false,
            trigger: Trigger::Scheduled,
            prune: true,
            only_if_due,
            ask: None,
            shutdown_after: false,
        },
        &Progress::new(),
        &silent,
    )
}

#[test]
fn interval_schedule_runs_only_when_due() {
    let mut e = env();
    e.profile.schedule.trigger = flux_core::profile::Trigger::Interval;
    e.profile.schedule.every = Some(flux_core::HumanDuration::from_secs(3600));
    // No previous run: due.
    assert_eq!(scheduled(&e, true).record.status, RunStatus::Completed);
    // Just ran: not due; nothing is recorded.
    let r = scheduled(&e, true);
    assert_eq!(r.record.status, RunStatus::Skipped);
    assert!(matches!(&r.outcome, Err(RunError::Skipped(w)) if w.starts_with("not due until")));
    assert_eq!(runlog::recent_runs(&e.paths, &e.id, 10).len(), 1);
    // Without --if-due it runs regardless.
    let r = scheduled(&e, false);
    assert_eq!(
        r.record.status,
        RunStatus::Unchanged,
        "{:?}",
        r.outcome.err()
    );
}

#[test]
fn second_run_of_same_profile_is_refused() {
    let e = env();
    let _held = flux_runner::jobs::try_lock(&e.paths, &e.id)
        .unwrap()
        .unwrap();
    let r = e.run();
    assert!(matches!(r.outcome, Err(RunError::Busy(_))));
    assert!(runlog::recent_runs(&e.paths, &e.id, 10).is_empty());
}

#[test]
fn paused_scheduled_backups_are_skipped() {
    let e = env();
    flux_runner::prefs::set_pause(&e.paths, None).unwrap();
    let r = scheduled(&e, false);
    assert_eq!(r.record.status, RunStatus::Skipped);
    assert!(matches!(&r.outcome, Err(RunError::Skipped(w)) if w == "scheduled backups are paused"));
    // Backing up by hand still works.
    assert_eq!(e.run().record.status, RunStatus::Completed);
    flux_runner::prefs::resume(&e.paths).unwrap();
    let r = scheduled(&e, false);
    assert_eq!(
        r.record.status,
        RunStatus::Unchanged,
        "{:?}",
        r.outcome.err()
    );
}

#[test]
fn vault_space_is_reclaimed_weekly_after_scheduled_backups() {
    let mut e = env();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    assert_eq!(scheduled(&e, false).record.status, RunStatus::Completed);
    // The first scheduled backup starts the clock; pretend it was a week ago.
    let state = e.paths.state_dir.join("maintenance/t.json");
    let week_ago = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * 8);
    fs::write(&state, format!(r#"{{"freed":"{week_ago}"}}"#)).unwrap();
    fs::write(e.root.join("src/f"), "changed").unwrap();
    let r = scheduled(&e, false);
    assert_eq!(
        r.record.status,
        RunStatus::Completed,
        "{:?}",
        r.outcome.err()
    );
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(
        events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.starts_with("weekly upkeep: freed"))),
        "{events:?}"
    );
    let saved = fs::read_to_string(&state).unwrap();
    assert!(
        !saved.contains(&week_ago.to_string()),
        "the clock restarted"
    );
}

/// "One backup at a time": a backup that starts while another holds the turn waits for
/// it (in the Waiting phase), then runs; stopped while waiting, it ends as cancelled.
#[test]
fn one_backup_at_a_time_waits_its_turn() {
    use flux_core::progress::Phase;
    use std::sync::Arc;
    use std::time::Duration;

    let e = env();
    let mut s = flux_runner::prefs::Settings::load(&e.paths);
    s.one_at_a_time = true;
    s.save(&e.paths).unwrap();
    let job = |progress: &Progress| {
        run_backup(
            &BackupJob {
                paths: &e.paths,
                id: &e.id,
                profile: &e.profile,
                name: None,
                dry_run: false,
                trigger: Trigger::Manual,
                prune: false,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            progress,
            &silent,
        )
    };
    let wait_for = |p: &Progress, phase: Phase| {
        for _ in 0..100 {
            if p.phase() == phase {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("never reached {phase:?}");
    };

    // Another backup has the turn: this one waits, then runs once it's free.
    let other = flux_runner::jobs::try_turn(&e.paths).unwrap().unwrap();
    let progress = Arc::new(Progress::new());
    std::thread::scope(|sc| {
        let p = progress.clone();
        let h = sc.spawn(move || job(&p));
        wait_for(&progress, Phase::Waiting);
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(progress.phase(), Phase::Waiting, "still waiting");
        drop(other);
        let r = h.join().unwrap();
        assert_eq!(
            r.record.status,
            RunStatus::Completed,
            "{:?}",
            r.outcome.err()
        );
    });

    // Stopped while waiting.
    let other = flux_runner::jobs::try_turn(&e.paths).unwrap().unwrap();
    let progress = Arc::new(Progress::new());
    std::thread::scope(|sc| {
        let p = progress.clone();
        let h = sc.spawn(move || job(&p));
        wait_for(&progress, Phase::Waiting);
        progress.cancel();
        let r = h.join().unwrap();
        assert_eq!(r.record.status, RunStatus::Cancelled);
    });
    drop(other);

    // Off (the default): no waiting at all.
    let mut s = flux_runner::prefs::Settings::load(&e.paths);
    s.one_at_a_time = false;
    s.save(&e.paths).unwrap();
    let _other = flux_runner::jobs::try_turn(&e.paths).unwrap().unwrap();
    assert!(job(&Progress::new()).outcome.is_ok());
}
