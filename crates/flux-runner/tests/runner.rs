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
/// Weekly upkeep gives space back; it never decides which snapshots go. After a failed
/// run every snapshot is kept, upkeep or not.
#[test]
fn weekly_upkeep_forgets_no_snapshot() {
    let mut e = env();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    e.profile.retention = flux_core::profile::Retention {
        keep_last: 1,
        keep_within: None,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
        keep_yearly: 0,
        safety_floor: 0,
        ..Default::default()
    };
    e.profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    let snapshots = |e: &Env| {
        let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
        repo.snapshots().unwrap().len()
    };
    assert_eq!(scheduled(&e, false).record.status, RunStatus::Completed);
    // A failed run (the drive "unplugged"), then upkeep falls due.
    fs::rename(e.root.join("tgt"), e.root.join("tgt-away")).unwrap();
    assert_eq!(scheduled(&e, false).record.status, RunStatus::Failed);
    fs::rename(e.root.join("tgt-away"), e.root.join("tgt")).unwrap();
    let week_ago = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * 8);
    fs::write(
        e.paths.state_dir.join("maintenance/t.json"),
        format!(r#"{{"freed":"{week_ago}"}}"#),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(e.root.join("src/f"), "changed").unwrap();
    let r = scheduled(&e, false);
    assert_eq!(r.record.status, RunStatus::Completed);
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    let said = |start: &str| {
        events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.starts_with(start)))
    };
    assert!(said("keeping every snapshot this time"), "{events:?}");
    assert!(said("weekly upkeep: freed"), "upkeep ran: {events:?}");
    assert_eq!(snapshots(&e), 2, "and forgot nothing");
}

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

/// `flux mount` reads a Vault's sparse file through its holes: zeros where the source had
/// holes, the data where it had data, and reads across the edges stitch both.
#[test]
fn mounted_vault_files_read_through_holes() {
    use std::os::unix::fs::FileExt;
    use std::sync::Arc;
    let mut e = env();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    let img = e.root.join("src/disk.img");
    let f = fs::File::create(&img).unwrap();
    f.set_len(24 << 20).unwrap();
    let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    f.write_all_at(&data, 8 << 20).unwrap();
    drop(f);
    if !flux_fs::fsops::is_sparse(&fs::metadata(&img).unwrap()) {
        eprintln!("this filesystem doesn't make sparse files; skipping");
        return;
    }
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let repo =
        Arc::new(flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap());
    let snap = repo.snapshots().unwrap().pop().unwrap();
    let view = flux_runner::view::SnapshotView::load(&repo, snap).unwrap();
    let i = view
        .entries
        .iter()
        .position(|x| x.path.ends_with(b"disk.img"))
        .unwrap();
    let mut file = view.open(i).unwrap();
    let source = fs::read(&img).unwrap();
    for (off, len) in [
        (0u64, 4096usize),         // in the leading hole
        ((8 << 20) - 100, 300),    // across the hole's end into data
        ((8 << 20) + 19_900, 300), // across the data's end into the hole
        (8 << 20, 20_000),         // exactly the data
        ((24 << 20) - 10, 100),    // past the end: cut short
        (3 << 20, 9 << 20),        // a long read over everything
    ] {
        let got = file.read_at(off, len).unwrap();
        let end = (off as usize + len).min(source.len());
        assert_eq!(got, source[off as usize..end], "read of {len} at {off}");
    }
}

/// After a backup with problems, the next backup keeps every snapshot; after a clean one,
/// pruning resumes. And the history can be kept shorter than the snapshots.
#[test]
fn pruning_holds_after_problems() {
    let mut e = env();
    e.profile.retention = flux_core::profile::Retention {
        keep_last: 1,
        keep_within: None,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
        keep_yearly: 0,
        safety_floor: 0,
        ..Default::default()
    };
    // Exact pruning: without unusual-change detection (which keeps one more).
    e.profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    let snapshots = |e: &Env| {
        let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
        repo.snapshots().unwrap().len()
    };
    let change = |e: &Env, n: u32| {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(e.root.join("src/f"), format!("version {n}")).unwrap();
    };
    assert_eq!(e.run().record.status, RunStatus::Completed);
    change(&e, 2);
    assert_eq!(e.run().record.status, RunStatus::Completed);
    assert_eq!(snapshots(&e), 1, "a clean history prunes as usual");

    // The drive "unplugged": a failed run.
    fs::rename(e.root.join("tgt"), e.root.join("tgt-away")).unwrap();
    assert_eq!(e.run().record.status, RunStatus::Failed);
    fs::rename(e.root.join("tgt-away"), e.root.join("tgt")).unwrap();
    change(&e, 3);
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Completed);
    assert_eq!(snapshots(&e), 2, "after a failure, nothing is pruned");
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(
        events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.starts_with("keeping every snapshot this time: the last backup failed"))),
        "{events:?}"
    );
    change(&e, 4);
    e.run();
    assert_eq!(snapshots(&e), 1, "after a clean run, pruning resumes");

    // Prune as usual, when asked.
    e.profile.retention.after_problems = flux_core::profile::AfterProblems::Prune;
    fs::rename(e.root.join("tgt"), e.root.join("tgt-away")).unwrap();
    e.run();
    fs::rename(e.root.join("tgt-away"), e.root.join("tgt")).unwrap();
    change(&e, 5);
    e.run();
    assert_eq!(snapshots(&e), 1);

    // History: keep the last 2 runs; their events go with them.
    let before = runlog::recent_runs(&e.paths, &e.id, 100);
    assert!(before.len() >= 6);
    e.profile.retention.keep_history_runs = Some(2);
    change(&e, 6);
    e.run();
    let after = runlog::recent_runs(&e.paths, &e.id, 100);
    assert_eq!(after.len(), 2);
    let dir = runlog::profile_log_dir(&e.paths, &e.id);
    assert!(
        !dir.join(format!("{}.jsonl", before.last().unwrap().run_id))
            .exists()
    );
    assert!(runlog::run_events(&e.paths, &e.id, &after[0].run_id).len() > 1);
}

/// Restore tests: due after a backup, they restore a sample and check it against the
/// snapshot's record; damage is found (a file changed in a Mirror snapshot, a damaged
/// Vault pack), and a source file changed since is a note, not a failure.
#[test]
fn restore_tests_find_damage() {
    use flux_core::profile::{ChangeDetection, RestoreTest, TestEvery, TestMode};
    use flux_runner::restore_test::{self, Problem, TestResult};
    let mut e = env();
    for i in 0..20 {
        fs::write(
            e.root.join(format!("src/file{i}.txt")),
            format!("contents of file {i}"),
        )
        .unwrap();
    }
    e.profile.options.change_detection = ChangeDetection::Checksum; // checksums recorded
    e.profile.restore_test = RestoreTest {
        every: TestEvery::Backup,
        mode: TestMode::Full,
        ..RestoreTest::default()
    };
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Completed);
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(
        events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.starts_with("restore test passed: 21 files"))),
        "{events:?}"
    );
    let st = restore_test::load_state(&e.paths, &e.id);
    assert!(st.last_passed.is_some() && st.history.len() == 1);

    // Damage a file inside the snapshot, keeping its size: the checksum catches it.
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snap = repo.snapshots().unwrap().pop().unwrap();
    let mirror = flux_mirror::open_repo(&e.id, &e.profile).unwrap();
    let victim = mirror
        .data_dir(&snap.id)
        .join(e.root.join("src/file3.txt").strip_prefix("/").unwrap());
    let len = fs::metadata(&victim).unwrap().len() as usize;
    // Snapshot files are read-only.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&victim, "X".repeat(len)).unwrap();
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &e.profile.restore_test,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    );
    assert_eq!(rep.result, TestResult::Failed, "{rep:?}");
    assert_eq!(rep.problems[0].problem, Problem::ContentDiffers);
    assert!(rep.problems[0].path.ends_with("src/file3.txt"));
    assert!(restore_test::summary(&rep).contains("restore test FAILED: 1 of 21 files"));

    // A source file changed since the backup: with compare_with_source, a note.
    fs::write(&victim, "contents of file 3").unwrap();
    fs::write(e.root.join("src/file5.txt"), "changed since").unwrap();
    let compare = RestoreTest {
        compare_with_source: true,
        ..e.profile.restore_test.clone()
    };
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &compare,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    );
    assert_eq!(rep.result, TestResult::Warning, "{rep:?}");
    assert!(
        rep.problems
            .iter()
            .all(|p| p.problem == Problem::DiffersFromSource)
    );
    assert!(rep.problems.iter().any(|p| p.path.ends_with("file5.txt")));
    // Limits stop it early, with a warning.
    let limited = RestoreTest {
        mode: TestMode::RandomFiles,
        files: 20,
        max_read: Some("1B".parse().unwrap()),
        ..RestoreTest::default()
    };
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &limited,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    );
    assert!(
        rep.stopped_early && rep.result == TestResult::Warning && rep.tested < 20,
        "{rep:?}"
    );

    // Not due again until the interval has passed.
    e.profile.restore_test.every = TestEvery::Week;
    assert!(!restore_test::due(
        &e.paths,
        &e.id,
        &e.profile.restore_test,
        jiff::Timestamp::now()
    ));
}

#[test]
fn restore_tests_of_a_damaged_vault_fail() {
    use flux_core::profile::{RestoreTest, TestMode};
    use flux_runner::restore_test::{self, Problem, TestResult};
    let mut e = env();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
    fs::write(e.root.join("src/big.bin"), &big).unwrap();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    assert_eq!(e.run().record.status, RunStatus::Completed);
    // Flip a byte in the biggest pack: the file's data (the folder listings are small).
    let data = e.root.join("tgt/flux-vault/data");
    let biggest = fs::read_dir(&data)
        .unwrap()
        .flatten()
        .flat_map(|d| fs::read_dir(d.path()).unwrap().flatten().map(|f| f.path()))
        .max_by_key(|p| fs::metadata(p).unwrap().len())
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&biggest, fs::Permissions::from_mode(0o600)).unwrap();
    let mut b = fs::read(&biggest).unwrap();
    let mid = b.len() / 3;
    b[mid] ^= 0xff;
    fs::write(&biggest, b).unwrap();
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let full = RestoreTest {
        mode: TestMode::Full,
        ..RestoreTest::default()
    };
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &full,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    );
    assert_eq!(rep.result, TestResult::Failed, "{rep:?}");
    assert!(
        rep.problems
            .iter()
            .any(|p| matches!(p.problem, Problem::CouldNotRestore | Problem::SizeDiffers)),
        "{rep:?}"
    );
    assert!(
        restore_test::load_state(&e.paths, &e.id)
            .last_passed
            .is_none()
    );
}

/// Simulated ransomware: files renamed to .locked and ransom notes. The backup is flagged,
/// its snapshot tagged, the one before pinned (with a note), pruning holds off; "This was
/// me" dismisses it.
#[test]
fn unusual_changes_are_flagged() {
    let mut e = env();
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..60 {
        fs::write(
            docs.join(format!("letter{i}.odt")),
            format!("letter number {i}"),
        )
        .unwrap();
    }
    e.profile.retention = flux_core::profile::Retention {
        keep_last: 1,
        keep_within: None,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
        keep_yearly: 0,
        safety_floor: 0,
        ..Default::default()
    };
    assert!(e.run().record.unusual.is_none(), "a first backup");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..30 {
        let f = docs.join(format!("letter{i}.odt"));
        fs::rename(&f, f.with_extension("odt.locked")).unwrap();
    }
    for dir in ["", "docs/", "docs/"] {
        let _ = fs::write(e.root.join(format!("src/{dir}HOW_TO_DECRYPT.txt")), "pay");
    }
    fs::write(e.root.join("src/README_RESTORE_FILES.txt"), "pay").unwrap();
    let r = e.run();
    let u = r.record.unusual.clone().expect("flagged");
    assert!(
        u.signals
            .iter()
            .any(|s| s == "30 files renamed to end in .locked"),
        "{:?}",
        u.signals
    );
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snaps = repo.snapshots().unwrap();
    assert_eq!(snaps.len(), 2);
    assert!(snaps[0].pinned, "the snapshot before is kept");
    assert!(
        snaps[0]
            .note
            .as_deref()
            .unwrap()
            .starts_with(flux_runner::AUTO_PIN_NOTE)
    );
    assert_eq!(snaps[1].tags, [flux_runner::UNUSUAL_TAG]);

    // The next backup removes nothing (keep_last = 1 would remove the middle one).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("letter59.odt"), "edited").unwrap();
    let r = e.run();
    assert!(r.record.unusual.is_none());
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Info { message } if message.contains("found unusual changes"))), "{events:?}");
    assert_eq!(repo.snapshots().unwrap().len(), 3);

    // "This was me": the next backup prunes again. What stays: the pinned one, and the
    // newest two (detection keeps the one before the newest for one more run).
    flux_runner::dismiss_unusual(&e.paths, &e.id, &u.snapshot).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("letter58.odt"), "edited too").unwrap();
    e.run();
    let left = repo.snapshots().unwrap();
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(left[0].pinned && !left[1].pinned);
}

/// A tiny local web server that records each request (method, path, body), answering 200.
/// What the listener heard: (method, path, body).
type Heard = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

fn listener() -> (String, Heard) {
    use std::io::{BufRead, BufReader, Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = got.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let mut parts = first.split_whitespace();
            let (m, p) = (
                parts.next().unwrap_or("").to_owned(),
                parts.next().unwrap_or("").to_owned(),
            );
            seen.lock()
                .unwrap()
                .push((m, p, String::from_utf8_lossy(&body).into_owned()));
            let mut s = s;
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (url, got)
}

#[test]
fn webhooks_hear_how_backups_go() {
    use flux_core::webhook::{HookEvent, Preset, Webhook};
    let (url, got) = listener();
    let mut e = env();
    e.profile.webhooks = vec![
        Webhook {
            url: format!("{url}/json"),
            preset: Preset::Json,
            events: vec![HookEvent::Started, HookEvent::Succeeded, HookEvent::Failed],
            headers: vec!["X-Token: secret".into()],
            enabled: true,
        },
        Webhook {
            url: format!("{url}/ping"),
            preset: Preset::Healthchecks,
            events: Vec::new(),
            headers: Vec::new(),
            enabled: true,
        },
    ];
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let reqs = got.lock().unwrap().clone();
    let paths: Vec<&str> = reqs.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(
        paths,
        ["/json", "/ping/start", "/json", "/ping"],
        "{reqs:?}"
    );
    let done: serde_json::Value = serde_json::from_str(&reqs[2].2).unwrap();
    assert_eq!(done["event"], "succeeded");
    assert_eq!(done["profile"], "t");
    assert!(
        done["message"]
            .as_str()
            .unwrap()
            .starts_with("Backup of “Test” finished: snapshot")
    );

    // A failure: the JSON hook hears "failed", the ping goes to /fail with the message.
    got.lock().unwrap().clear();
    fs::rename(e.root.join("tgt"), e.root.join("tgt-away")).unwrap();
    assert_eq!(e.run().record.status, RunStatus::Failed);
    let reqs = got.lock().unwrap().clone();
    let fail: Vec<_> = reqs.iter().filter(|r| r.1 == "/ping/fail").collect();
    assert_eq!(fail.len(), 1, "{reqs:?}");
    assert!(fail[0].2.contains("failed"));
    let j = reqs.iter().rfind(|r| r.1 == "/json").unwrap();
    let v: serde_json::Value = serde_json::from_str(&j.2).unwrap();
    assert_eq!(v["event"], "failed");
    assert!(v["error"].as_str().unwrap().contains("does not exist"));
    fs::rename(e.root.join("tgt-away"), e.root.join("tgt")).unwrap();

    // A webhook nobody answers: the backup still works, and the log says so.
    e.profile.webhooks = vec![Webhook {
        url: "http://127.0.0.1:9/nothing".into(),
        preset: Preset::Json,
        events: vec![HookEvent::Succeeded],
        headers: Vec::new(),
        enabled: true,
    }];
    fs::write(e.root.join("src/f"), "changed").unwrap();
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Completed);
    let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
    assert!(
        events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Warning { message, .. } if message.starts_with("webhook http://127.0.0.1:9/nothing:"))),
        "{events:?}"
    );
}

#[test]
fn runs_record_where_the_time_went() {
    let mut e = env();
    fs::write(e.root.join("src/big"), vec![7u8; 3 << 20]).unwrap();
    // Mirror: the phases.
    let r = e.run();
    let perf = r.record.performance.expect("recorded");
    let names: Vec<_> = perf.phases.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"Scanning") && names.contains(&"Copying"),
        "{names:?}"
    );
    assert!(perf.phases.iter().all(|p| p.secs >= 0.0));
    // Vault: the stages of the work too, and the storage's writes.
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    e.profile.restore_test.every = flux_core::profile::TestEvery::Backup;
    let r = e.run();
    assert_eq!(
        r.record.status,
        RunStatus::Completed,
        "{:?}",
        r.record.error
    );
    let perf = r.record.performance.expect("recorded");
    let stages: Vec<_> = perf.stages.iter().map(|s| s.name.as_str()).collect();
    for s in [
        "Reading files",
        "Finding what's already stored",
        "Compressing and encrypting",
        "Writing and uploading",
    ] {
        assert!(stages.contains(&s), "{s} in {stages:?}");
    }
    assert!(perf.writes > 0 && perf.bytes_written > 0 && perf.write_ms.is_some());
    assert!(perf.throughput.is_some_and(|t| t > 0.0));
    let names: Vec<_> = perf.phases.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"Testing a restore"), "{names:?}");
    assert_eq!(
        names.iter().filter(|n| **n == "Scanning").count(),
        1,
        "the restore test's own scan isn't the backup's: {names:?}"
    );
    // Kept in the run log.
    let runs = runlog::recent_runs(&e.paths, &e.id, 10);
    assert!(runs.last().unwrap().performance.is_some());
}

#[test]
fn storage_says_what_dedup_and_compression_save() {
    // Vault: the same file twice is stored once; text compresses.
    let mut e = env();
    let text = "the same line again\n".repeat(50_000);
    fs::write(e.root.join("src/a.txt"), &text).unwrap();
    fs::write(e.root.join("src/b.txt"), &text).unwrap();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    e.profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let s = repo.storage().unwrap();
    let both = 2 * text.len() as u64 + 4;
    assert_eq!(
        (s.snapshots, s.latest_bytes, s.logical_bytes),
        (1, both, both)
    );
    assert!(s.dedup_saved.unwrap() >= text.len() as u64, "{s:?}");
    assert!(s.compression_saved.unwrap() > 0, "{s:?}");
    assert!(s.stored_bytes.unwrap() < text.len() as u64 / 10, "{s:?}");
    assert_eq!(s.shared_with, 0);

    // Mirror: a second snapshot shares the unchanged files with the first.
    let mut m = env();
    fs::write(m.root.join("src/big"), vec![1u8; 1 << 20]).unwrap();
    m.profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    assert_eq!(m.run().record.status, RunStatus::Completed);
    fs::write(m.root.join("src/f"), "data, changed").unwrap();
    assert_eq!(m.run().record.status, RunStatus::Completed);
    let repo = flux_runner::repo::AnyRepo::open(&m.paths, &m.id, &m.profile, None).unwrap();
    let s = repo.storage().unwrap();
    assert_eq!(s.snapshots, 2);
    assert!(s.logical_bytes >= 2 << 20);
    assert!(
        s.stored_bytes.unwrap() < (1 << 20) + 100,
        "the big file once: {s:?}"
    );
    assert!(s.dedup_saved.unwrap() >= 1 << 20);
    assert!(s.compression_saved.is_none());
}

/// Whether a Vault is encrypted is written in the Vault, where whoever holds the drive
/// can change it. A backup set to be encrypted must not go on in the clear.
#[test]
fn an_encrypted_backup_never_writes_to_a_vault_that_says_it_is_not() {
    let mut e = env();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let snapshots = || {
        fs::read_dir(e.root.join("tgt/flux-vault/snapshots"))
            .unwrap()
            .count()
    };
    assert_eq!(snapshots(), 1);

    // The same Vault, as a backup that is meant to be encrypted finds it.
    let pw = e.root.join("pw");
    fs::write(&pw, "secret\n").unwrap();
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: true,
        password_source: flux_core::profile::PasswordSource::File { path: pw },
        ..Default::default()
    });
    fs::write(e.root.join("src/private"), "not for the server").unwrap();
    let r = e.run();
    assert_eq!(r.record.status, RunStatus::Failed);
    let err = r.outcome.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("says it is not"), "{err}");
    assert_eq!(snapshots(), 1, "nothing was written");
}

/// A password file with nothing in it (or a command that printed nothing) must not set up
/// "encryption" that opens without a password.
#[test]
fn an_empty_password_file_creates_nothing() {
    for engine in [flux_core::Engine::Vault, flux_core::Engine::Borg] {
        let mut e = env();
        let pw = e.root.join("pw");
        fs::write(&pw, "\n").unwrap();
        let source = flux_core::profile::PasswordSource::File { path: pw };
        e.profile.engine = engine;
        match engine {
            flux_core::Engine::Vault => {
                e.profile.vault = Some(flux_core::profile::VaultOptions {
                    password_source: source,
                    ..Default::default()
                });
            }
            _ => {
                e.profile.borg = Some(flux_core::profile::BorgOptions {
                    password_source: source,
                    ..Default::default()
                });
            }
        }
        let r = e.run();
        assert_eq!(r.record.status, RunStatus::Failed, "{engine}");
        let err = r.outcome.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("is empty"), "{engine}: {err}");
        assert!(
            fs::read_dir(e.root.join("tgt")).unwrap().next().is_none(),
            "{engine}: nothing was created on the target"
        );
    }
}

/// What a newer Flux wrote is refused, saying so, not misread.
#[test]
fn newer_formats_are_refused_with_a_clear_message() {
    let open_err =
        |e: &Env| match flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None) {
            Ok(r) => r
                .snapshots()
                .map(|_| String::new())
                .unwrap_or_else(|e| e.to_string()),
            Err(e) => e.to_string(),
        };
    // A Vault of format 3.
    let mut v = env();
    v.profile.engine = flux_core::Engine::Vault;
    v.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    assert_eq!(v.run().record.status, RunStatus::Completed);
    let config = v.root.join("tgt/flux-vault/config");
    let mut c: serde_json::Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    c["format"] = 3.into();
    fs::write(&config, serde_json::to_vec(&c).unwrap()).unwrap();
    let err = open_err(&v);
    assert!(err.contains("newer than this Flux supports"), "{err}");

    // A Mirror repository, a snapshot's meta.json and a manifest from the future.
    let m = env();
    assert_eq!(m.run().record.status, RunStatus::Completed);
    let repo = fs::read_dir(m.root.join("tgt/flux-backup"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let root = fs::read_dir(&repo)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("t");
    let snap = fs::read_dir(root.join("snapshots"))
        .unwrap()
        .flatten()
        .find(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .unwrap()
        .path();
    let meta = snap.join("meta.json");
    let original = fs::read(&meta).unwrap();
    let mut j: serde_json::Value = serde_json::from_slice(&original).unwrap();
    j["format"] = 2.into();
    fs::write(&meta, serde_json::to_vec(&j).unwrap()).unwrap();
    // Listed snapshots skip it (one odd snapshot never hides the others); read, it says why.
    let r = flux_runner::repo::AnyRepo::open(&m.paths, &m.id, &m.profile, None).unwrap();
    assert!(r.snapshots().unwrap().is_empty(), "not misread");
    let err = flux_mirror::repo::read_meta(&snap).unwrap_err().to_string();
    assert!(err.contains("newer Flux"), "{err}");
    fs::write(&meta, &original).unwrap();

    let manifest = snap.join("manifest.zst");
    let mut plain = zstd_decode(&fs::read(&manifest).unwrap());
    plain[8..12].copy_from_slice(&2u32.to_le_bytes());
    fs::write(&manifest, zstd_encode(&plain)).unwrap();
    let r = flux_runner::repo::AnyRepo::open(&m.paths, &m.id, &m.profile, None).unwrap();
    let s = r.find("latest").unwrap();
    let err = match r.entries(&s) {
        Ok(mut it) => it
            .find_map(|e| e.err())
            .map(|e| e.to_string())
            .unwrap_or_default(),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("newer Flux"), "{err}");

    let marker = root.join("flux-repo.toml");
    let text = fs::read_to_string(&marker)
        .unwrap()
        .replace("format = 1", "format = 2");
    fs::write(&marker, text).unwrap();
    let err = open_err(&m);
    assert!(err.contains("format 2"), "{err}");

    // A profile of schema 2.
    let mut p = m.profile.clone();
    p.version = 2;
    let issues = p.validate();
    assert!(
        issues.iter().any(|i| i.message.contains("newer Flux")),
        "{issues:?}"
    );
}

fn zstd_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut zstd::stream::read::Decoder::new(b).unwrap(), &mut out)
        .unwrap();
    out
}

fn zstd_encode(b: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(b, 3).unwrap()
}
