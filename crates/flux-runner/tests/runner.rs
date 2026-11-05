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
    assert_eq!(scheduled(&e, false).record.status, RunStatus::Unchanged);
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
