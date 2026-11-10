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
    assert_eq!(rep.guarantee, None, "cut short: nothing is claimed");

    // Sized by what it must prove: damage to 20% of the 21 files (5 or more), 99% sure.
    let sure = RestoreTest {
        mode: TestMode::Confidence,
        damage: "20%".parse().unwrap(),
        ..RestoreTest::default()
    };
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &sure,
        &repo,
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    );
    let n = flux_runner::sample::sample_size(21, sure.damage, sure.certainty);
    assert!(n < 21 && flux_runner::sample::miss_probability(21, 5, n) <= 0.01);
    assert_eq!((rep.result, rep.tested), (TestResult::Passed, n), "{rep:?}");
    let g = rep.guarantee.expect("a passing sample says what it proves");
    assert_eq!((g.files, g.damaged), (21, 5));
    assert!(
        restore_test::summary(&rep).ends_with(
            "if 20% or more of its 21 files were damaged, this test would have found it (99% certain)"
        ),
        "{}",
        restore_test::summary(&rep)
    );
    // And it does find it: five damaged files, twenty tests, no miss expected (each
    // misses with a chance under 1%; all twenty passing by luck: under 10^-40).
    for i in 10..15 {
        let f = mirror.data_dir(&snap.id).join(
            e.root
                .join(format!("src/file{i}.txt"))
                .strip_prefix("/")
                .unwrap(),
        );
        let len = fs::metadata(&f).unwrap().len() as usize;
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&f, "X".repeat(len)).unwrap();
    }
    let caught = (0..20)
        .filter(|_| {
            restore_test::run(
                &e.paths,
                &e.id,
                &e.profile,
                &sure,
                &repo,
                &flux_core::progress::Progress::new(),
                &flux_core::progress::silent,
            )
            .result
                == TestResult::Failed
        })
        .count();
    assert!(caught >= 19, "caught {caught} of 20");

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

/// Every file under `dir` (paths relative to it), to see what a step wrote.
fn files_under(dir: &std::path::Path) -> std::collections::BTreeSet<String> {
    fn walk(
        base: &std::path::Path,
        d: &std::path::Path,
        out: &mut std::collections::BTreeSet<String>,
    ) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(base, &p, out);
            } else {
                out.insert(p.strip_prefix(base).unwrap().display().to_string());
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(dir, dir, &mut out);
    out
}

/// A restore test leaves nothing behind (1.1): the files are read back from an encrypted
/// Vault and checked as they stream, never restored anywhere, so the only file it writes
/// is its own report. A file bigger than one piece is checked whole, and a temporary
/// folder a 1.0 restore test left behind (plain copies of backed-up files) is removed.
#[test]
fn restore_tests_leave_nothing_behind() {
    use flux_core::profile::{ChangeDetection, RestoreTest, TestMode};
    use flux_runner::restore_test::{self, TestResult};
    let mut e = env();
    let big: Vec<u8> = (0..3_500_000u32).map(|i| (i * 7 % 253) as u8).collect();
    fs::write(e.root.join("src/big.bin"), &big).unwrap();
    fs::write(e.root.join("src/secret.txt"), "the plans").unwrap();
    let pw = e.root.join("pw");
    fs::write(&pw, "correct horse\n").unwrap();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.options.change_detection = ChangeDetection::Checksum;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        password_source: flux_core::profile::PasswordSource::File { path: pw },
        ..Default::default()
    });
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let left = e.paths.cache_dir.join("restore-tests/t-1-2");
    fs::create_dir_all(&left).unwrap();
    fs::write(left.join("secret.txt"), "the plans").unwrap();

    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    repo.snapshots().unwrap();
    let before = files_under(&e.root);
    let full = RestoreTest {
        mode: TestMode::Full,
        compare_with_source: true,
        ..RestoreTest::default()
    };
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &full,
        &repo,
        &Progress::new(),
        &silent,
    );
    assert_eq!(rep.result, TestResult::Passed, "{rep:?}");
    assert_eq!(rep.tested, 3, "f, big.bin and secret.txt: {rep:?}");
    assert_eq!(rep.bytes, 3_500_000 + 9 + 4);
    let after = files_under(&e.root);
    let written: Vec<_> = after.difference(&before).collect();
    let removed: Vec<_> = before.difference(&after).collect();
    assert_eq!(
        written,
        ["home/state/restore-tests/t.json"],
        "only the report"
    );
    assert_eq!(
        removed,
        ["home/cache/restore-tests/t-1-2/secret.txt"],
        "and the folder a 1.0 test left behind is gone"
    );
    assert!(!e.paths.cache_dir.join("restore-tests").exists());

    // Damage found as before: a byte changed in the source since is only a note…
    fs::write(e.root.join("src/secret.txt"), "the PLANS").unwrap();
    let rep = restore_test::run(
        &e.paths,
        &e.id,
        &e.profile,
        &full,
        &repo,
        &Progress::new(),
        &silent,
    );
    assert_eq!(rep.result, TestResult::Warning, "{rep:?}");
    // …and a cancelled test stops without counting the file it was in.
    let p = Progress::new();
    p.cancel();
    let rep = restore_test::run(&e.paths, &e.id, &e.profile, &full, &repo, &p, &silent);
    assert!(rep.stopped_early && rep.tested == 0, "{rep:?}");
}

/// Borg's files stream out of `borg extract --stdout`: no temporary copy (with the real
/// borg, when it's installed).
#[test]
fn borg_restore_tests_stream() {
    use flux_core::profile::{RestoreTest, TestMode};
    use flux_runner::restore_test::{self, TestResult};
    let has_borg = std::process::Command::new("borg")
        .arg("--version")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("borg 1."));
    if !has_borg {
        eprintln!("skipped: no borg 1.x");
        return;
    }
    let mut e = env();
    fs::write(e.root.join("src/a.txt"), "alpha").unwrap();
    fs::write(e.root.join("src/b.bin"), vec![7u8; 1_500_000]).unwrap();
    e.profile.engine = flux_core::Engine::Borg;
    e.profile.borg = Some(flux_core::profile::BorgOptions {
        encryption: flux_core::profile::BorgEncryption::None,
        ..Default::default()
    });
    assert_eq!(e.run().record.status, RunStatus::Completed);
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    // The archive's file listing is cached as browsing caches it (names, no contents).
    let snap = repo.snapshots().unwrap().pop().unwrap();
    assert!(repo.entries(&snap).unwrap().count() >= 3);
    let before = files_under(&e.root.join("home"));
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
        &Progress::new(),
        &silent,
    );
    assert_eq!(rep.result, TestResult::Passed, "{rep:?}");
    assert_eq!((rep.tested, rep.bytes), (3, 4 + 5 + 1_500_000));
    let after = files_under(&e.root.join("home"));
    let written: Vec<_> = after.difference(&before).collect();
    assert_eq!(written, ["state/restore-tests/t.json"], "only the report");
}

/// Simulated ransomware: files renamed to .locked and ransom notes. Flux Sentry sees
/// them in the files before the backup is written: it isn't made, the newest snapshot is
/// pinned (with a note), and backups are on hold until it's settled.
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
    let flagged = r.record.clone();
    assert!(
        u.signals
            .iter()
            .any(|s| s == "30 files renamed to end in .locked"),
        "{:?}",
        u.signals
    );
    // Stopped before anything was written: no second snapshot, nothing of the attack
    // in the backups.
    assert!(u.stopped && u.snapshot.starts_with("before:"), "{u:?}");
    assert!(
        matches!(&r.outcome, Err(flux_runner::RunError::Held(why)) if why.starts_with("not backed up")),
        "{:?}",
        r.outcome
    );
    assert_eq!(r.record.status, RunStatus::Skipped);
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snaps = repo.snapshots().unwrap();
    assert_eq!(snaps.len(), 1);
    assert_eq!(u.before.as_deref(), Some(snaps[0].id.as_str()));
    assert!(snaps[0].pinned, "the snapshot before is kept");
    assert!(
        snaps[0]
            .note
            .as_deref()
            .unwrap()
            .starts_with(flux_runner::AUTO_PIN_NOTE)
    );

    // On hold: nothing more is backed up, by hand or by schedule, until it's settled.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("letter59.odt"), "edited").unwrap();
    let r = e.run();
    assert!(
        matches!(&r.outcome, Err(flux_runner::RunError::Held(why)) if why.contains("on hold") && why.contains("renamed to end in .locked")),
        "{:?}",
        r.outcome
    );
    assert_eq!(r.record.status, RunStatus::Skipped);
    assert_eq!(repo.snapshots().unwrap().len(), 1, "no snapshot was made");
    assert!(flux_runner::sentry_hold(&e.paths, &e.id).is_some());

    // "This was me": backups go on, and the pin comes off. The next backup takes those
    // very changes without being stopped for them again.
    flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, true, Some(&repo))
        .unwrap();
    assert!(flux_runner::sentry_hold(&e.paths, &e.id).is_none());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r = e.run();
    assert!(
        matches!(
            r.outcome,
            Ok(flux_fs::outcome::BackupOutcome::Created { .. })
        ),
        "{:?}",
        r.outcome
    );
    assert!(r.record.unusual.is_none());
    assert!(
        files_in(&repo)
            .iter()
            .any(|p| p.ends_with("letter3.odt.locked"))
    );
    let left = repo.snapshots().unwrap();
    assert!(
        left.iter().all(|s| !s.pinned && s.tags.is_empty()),
        "{left:?}"
    );

    // Flux Sentry learns from a backup's ordinary runs, and from unusual ones only once
    // their owner has said they were theirs. Every such word is remembered, not only
    // the latest.
    let runs = || runlog::recent_runs(&e.paths, &e.id, 20);
    let learns_from = |runs: Vec<runlog::RunRecord>| {
        flux_runner::sentry_history(&e.paths, &e.id, runs)
            .iter()
            .any(|r| r.run_id == flagged.run_id)
    };
    assert!(learns_from(runs()), "dismissed: part of its normal");
    flux_runner::dismiss_unusual(&e.paths, &e.id, "another-snapshot").unwrap();
    assert!(flux_runner::unusual_dismissed(&e.paths, &e.id, &flagged));
    assert!(learns_from(runs()));
    // Not dismissed: left out.
    fs::remove_file(
        e.paths
            .state_dir
            .join(format!("unusual/{}.dismissed", e.id)),
    )
    .unwrap();
    assert!(!learns_from(runs()));
    assert_eq!(
        flux_runner::sentry_history(&e.paths, &e.id, runs()).len(),
        runs().len() - 1
    );
    // …and the alarm is open again, until it's dealt with: that lets backups go on
    // without teaching Flux Sentry anything. (It holds the backup only while no backup
    // has been made since: here one has, so the alarm's own run is asked.)
    let alarm = std::slice::from_ref(&flagged);
    assert!(flux_runner::open_unusual(&e.paths, &e.id, alarm).is_some());
    assert!(flux_runner::sentry_hold(&e.paths, &e.id).is_none());
    flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, false, None).unwrap();
    assert!(flux_runner::open_unusual(&e.paths, &e.id, alarm).is_none());
    assert!(!learns_from(runs()), "dealt with isn't \"mine\"");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("letter57.odt"), "edited again").unwrap();
    assert!(e.run().outcome.is_ok());
}

/// The look before a backup compares the files with the newest snapshot as a scan sees
/// them. A backup that records checksums (which a scan doesn't have) and one with a
/// source's drive unplugged mustn't look like a mass change.
#[test]
fn the_look_before_a_backup_raises_no_false_alarm() {
    use flux_core::profile::ChangeDetection;
    let mut e = env();
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    e.profile.options.change_detection = ChangeDetection::Checksum;
    assert!(e.run().outcome.is_ok());
    for round in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(docs.join("f0.txt"), format!("edited {round}")).unwrap();
        let r = e.run();
        assert!(
            matches!(
                r.outcome,
                Ok(flux_fs::outcome::BackupOutcome::Created { .. })
            ),
            "round {round}: {:?}",
            r.outcome
        );
        assert!(r.record.unusual.is_none());
        let events = runlog::run_events(&e.paths, &e.id, &r.record.run_id);
        assert!(
            !events.iter().any(|ev| matches!(&ev.event, flux_core::progress::Event::Warning { message, .. } if message.contains("unusual"))),
            "{events:?}"
        );
    }
    // And a real mass change is still stopped: 200 of the 300 files rewritten.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..200 {
        fs::write(
            docs.join(format!("f{i}.txt")),
            format!("overwritten, all of it, {i}"),
        )
        .unwrap();
    }
    let r = e.run();
    let u = r.record.unusual.expect("stopped");
    assert!(u.stopped, "{u:?}");
    assert!(
        u.signals[0].starts_with("200 files changed or deleted"),
        "{:?}",
        u.signals
    );
}

/// Tripwire files: planted by the first backup, looked at before every one; one that is
/// changed stops the backup, whatever else has (not) happened, and they are set again
/// when it's settled and removed when the setting is turned off.
#[test]
fn a_tripped_tripwire_stops_the_backup() {
    use flux_runner::tripwire;
    let mut e = env();
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..40 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file {i}")).unwrap();
    }
    e.profile.tripwires = true;
    assert!(e.run().outcome.is_ok());
    let wire = docs.join(tripwire::NAME);
    assert!(wire.is_file() && e.root.join("src").join(tripwire::NAME).is_file());
    assert_eq!(tripwire::load(&e.paths, &e.id).len(), 2);
    // An ordinary day: nothing.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f1.txt"), "edited").unwrap();
    let r = e.run();
    assert!(
        r.outcome.is_ok() && r.record.unusual.is_none(),
        "{:?}",
        r.outcome
    );
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    assert_eq!(repo.snapshots().unwrap().len(), 2);

    // One file encrypted in place, and it is the tripwire: far too little for any other
    // signal, and enough.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(&wire, [0x5a; 300]).unwrap();
    let r = e.run();
    assert!(
        matches!(r.outcome, Err(flux_runner::RunError::Held(_))),
        "{:?}",
        r.outcome
    );
    let u = r.record.unusual.expect("stopped");
    assert!(u.stopped);
    assert_eq!(
        u.signals,
        [format!("a tripwire file was changed ({})", wire.display())]
    );
    assert_eq!(u.affected, [wire.display().to_string()]);
    assert_eq!(repo.snapshots().unwrap().len(), 2, "nothing was written");
    assert!(repo.snapshots().unwrap()[1].pinned);

    // Dealt with: the tripwire is set again (the changed file moved aside, never
    // deleted), and backups go on.
    flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, false, None).unwrap();
    assert!(wire.is_file() && fs::read_to_string(&wire).unwrap().contains("tripwire"));
    assert_eq!(
        fs::read(docs.join(format!("{}.tripped", tripwire::NAME))).unwrap(),
        [0x5a; 300]
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r = e.run();
    assert!(
        r.outcome.is_ok() && r.record.unusual.is_none(),
        "{:?}",
        r.outcome
    );

    // Turned off: the next backup takes them away.
    e.profile.tripwires = false;
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f2.txt"), "edited").unwrap();
    assert!(e.run().outcome.is_ok());
    assert!(!wire.exists() && !e.root.join("src").join(tripwire::NAME).exists());
    assert!(tripwire::load(&e.paths, &e.id).is_empty());
}

/// The paths of the newest snapshot's files.
fn files_in(repo: &flux_runner::repo::AnyRepo) -> Vec<String> {
    let snap = repo.snapshots().unwrap().pop().unwrap();
    repo.entries(&snap)
        .unwrap()
        .map(|e| String::from_utf8_lossy(&e.unwrap().path).into_owned())
        .collect()
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

/// The main backup's drive is away, so Flux Sentry can't look at the files against its
/// snapshots, and the main backup fails. The other places must not take the files
/// unlooked-at for that: each is looked at against its own newest snapshot. A few changed
/// files go there as always; what looks like an attack is stopped before it's written.
#[test]
fn other_places_are_looked_at_when_the_main_backup_is_out_of_reach() {
    fn snapshots_in(dir: &std::path::Path) -> usize {
        let mut n = 0;
        for d in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = d.path();
            if p.is_dir() && !p.is_symlink() {
                if d.file_name() == "snapshots" {
                    n += fs::read_dir(&p)
                        .unwrap()
                        .flatten()
                        .filter(|s| !s.file_name().to_string_lossy().starts_with('.'))
                        .count();
                } else {
                    n += snapshots_in(&p);
                }
            }
        }
        n
    }
    let mut e = env();
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    let nas = e.root.join("nas");
    fs::create_dir_all(&nas).unwrap();
    e.profile
        .destinations
        .push(flux_core::profile::Destination {
            name: "nas".into(),
            label: None,
            engine: flux_core::Engine::Mirror,
            target: Target::Local {
                path: nas.clone(),
                fs_uuid: None,
            },
            offsite: true,
            mode: flux_core::profile::CopyMode::default(),
            vault: None,
            rsync: None,
            borg: None,
        });
    assert!(e.run().outcome.is_ok());
    assert_eq!(snapshots_in(&nas), 1);

    // The main drive goes away. An ordinary day's change still reaches the other place.
    fs::rename(e.root.join("tgt"), e.root.join("tgt-unplugged")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f0.txt"), "edited").unwrap();
    let r = e.run();
    assert!(r.outcome.is_err(), "the main backup can't be made");
    assert!(r.record.unusual.is_none(), "{:?}", r.record.unusual);
    assert_eq!(snapshots_in(&nas), 2);

    // 200 of the 300 files rewritten: nothing of it goes anywhere.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..200 {
        fs::write(
            docs.join(format!("f{i}.txt")),
            format!("overwritten, all of it, {i}"),
        )
        .unwrap();
    }
    let r = e.run();
    let u = r.record.unusual.expect("stopped");
    assert!(u.stopped, "{u:?}");
    assert_eq!(snapshots_in(&nas), 2, "the other place keeps what it had");
    assert_eq!(r.record.destinations.len(), 1);
    assert!(
        r.record.destinations[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Flux Sentry")),
        "{:?}",
        r.record.destinations
    );
    assert!(flux_runner::sentry_hold(&e.paths, &e.id).is_some());
}

fn have_rsync() -> bool {
    std::process::Command::new("rsync")
        .arg("--version")
        .output()
        .is_ok()
}

/// rsync and Borg backups are looked at after they're written, so the snapshot with the
/// damage in it is the newest one. Once the alarm is dealt with and the files are back,
/// the next backup must be compared with the snapshot from before the damage: against
/// the damaged one it would look like a second attack, and the damaged snapshot would
/// be pinned as "the one from before".
#[test]
fn a_clean_backup_after_a_dealt_with_alarm_is_not_flagged_again() {
    if !have_rsync() {
        eprintln!("skipping: no rsync");
        return;
    }
    let mut e = env();
    e.profile.engine = flux_core::Engine::Rsync;
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    let first = e.run();
    assert!(first.outcome.is_ok(), "{:?}", first.outcome);
    // The files as they were, times and all (what a restore puts back).
    let stash = e.root.join("stash");
    let cp = |from: &std::path::Path, to: &std::path::Path| {
        assert!(
            std::process::Command::new("cp")
                .arg("-a")
                .arg(from)
                .arg(to)
                .status()
                .unwrap()
                .success()
        );
    };
    cp(&docs, &stash);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..200 {
        fs::write(
            docs.join(format!("f{i}.txt")),
            format!("overwritten, all of it, {i}"),
        )
        .unwrap();
    }
    let r = e.run();
    let u = r.record.unusual.clone().expect("flagged");
    assert!(
        !u.stopped,
        "an rsync backup is looked at after it's written"
    );
    let before = u.before.clone().expect("a snapshot from before");

    // Dealt with: the files restored, the alarm settled.
    fs::remove_dir_all(&docs).unwrap();
    cp(&stash, &docs);
    flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, false, None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f299.txt"), "one ordinary edit").unwrap();
    let r = e.run();
    assert!(r.outcome.is_ok(), "{:?}", r.outcome);
    assert!(
        r.record.unusual.is_none(),
        "the clean backup, flagged: {:?}",
        r.record.unusual
    );
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snaps = repo.snapshots().unwrap();
    assert!(
        snaps.iter().find(|s| s.id == before).unwrap().pinned,
        "the snapshot from before stays the pinned one"
    );
    assert!(
        !snaps.iter().find(|s| s.id == u.snapshot).unwrap().pinned,
        "the damaged snapshot isn't kept as if it were the good one"
    );
    // What the look counted is what Sentry learns from.
    assert_eq!(r.record.sentry_seen.map(|s| s.touched), Some(1));
}

/// A source whose drive is away is left out of an rsync backup's snapshot (it can't
/// carry the files over). That is no deletion: 700 of 1000 files "gone" must not hold
/// the backup.
#[test]
fn a_source_left_out_because_its_drive_is_away_is_no_mass_deletion() {
    if !have_rsync() {
        eprintln!("skipping: no rsync");
        return;
    }
    let mut e = env();
    e.profile.engine = flux_core::Engine::Rsync;
    let docs = e.root.join("src/docs");
    let photos = e.root.join("photos");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(&photos).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    for i in 0..700 {
        fs::write(photos.join(format!("p{i}.jpg")), format!("photo {i}")).unwrap();
    }
    e.profile.sources.paths.push(photos.clone());
    assert!(e.run().outcome.is_ok());

    // The photos' drive "isn't connected": the folder is on another filesystem than
    // the one recorded for it.
    e.profile
        .sources
        .volumes
        .push(flux_core::profile::SourceVolume {
            path: photos.clone(),
            fs_uuid: "0000-NOT-THIS-ONE".into(),
            when_missing: flux_core::profile::WhenMissing::Skip,
            wait_for: None,
        });
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f0.txt"), "edited").unwrap();
    let r = e.run();
    assert!(r.outcome.is_ok(), "{:?}", r.outcome);
    assert_eq!(r.record.sources_missing, [photos]);
    assert!(
        r.record.unusual.is_none(),
        "left out, not deleted: {:?}",
        r.record.unusual.map(|u| u.signals)
    );
}

/// Flux Sentry learns from the last 60 backups that made a snapshot: runs that failed
/// or found nothing changed don't push them out of what it reads. And an alarm from
/// before other backups were made (Sentry was off meanwhile) isn't a hold any more.
#[test]
fn sentry_history_counts_backups_not_attempts() {
    use flux_runner::runlog::RunRecord;
    let e = env();
    assert!(e.run().outcome.is_ok());
    let made = runlog::recent_runs(&e.paths, &e.id, 1).remove(0);
    let with = |status: RunStatus, n: usize| -> Vec<RunRecord> {
        (0..n)
            .map(|_| RunRecord {
                status,
                ..made.clone()
            })
            .collect()
    };
    // Newest first: 80 failed attempts, then 10 backups.
    let mut runs = with(RunStatus::Failed, 80);
    runs.extend(with(RunStatus::Completed, 10));
    let history = flux_runner::sentry_history(&e.paths, &e.id, runs);
    assert_eq!(
        flux_runner::unusual::learn(&history).map(|b| b.runs()),
        Some(10),
        "the ten backups behind eighty failed attempts are what it knows"
    );

    // An alarm, then a backup made after it: nothing is held.
    let mut alarm = made.clone();
    alarm.unusual = Some(flux_runner::unusual::UnusualReport {
        signals: vec!["something".into()],
        affected: Vec::new(),
        affected_total: 0,
        by_folder: Vec::new(),
        by_extension: Vec::new(),
        snapshot: "then".into(),
        before: None,
        stopped: false,
        before_pinned: false,
        pinned_by_flux: false,
        sentry: None,
    });
    assert!(flux_runner::open_unusual(&e.paths, &e.id, std::slice::from_ref(&alarm)).is_some());
    let since = vec![made.clone(), alarm];
    assert!(flux_runner::open_unusual(&e.paths, &e.id, &since).is_none());
}

/// "This was me" takes off the pin Flux put on the snapshot from before, also when that
/// snapshot carries a note of its owner's (which stays), and says so truthfully when
/// the backups can't be reached. "It's dealt with" on a stopped backup lets the next
/// one through unlooked-at, and Sentry learns nothing from that one either.
#[test]
fn settling_takes_off_what_flux_put_on_and_learns_only_from_this_was_me() {
    let e = env();
    let docs = e.root.join("src/docs");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..300 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    assert!(e.run().outcome.is_ok());
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let first = repo.snapshots().unwrap().pop().unwrap();
    repo.change(
        &first.id,
        &flux_fs::snapshot::SnapshotChange {
            note: Some(Some("before the move".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let rewrite = |round: u32| {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        for i in 0..200 {
            fs::write(
                docs.join(format!("f{i}.txt")),
                format!("overwritten, all of it, {i}, round {round}"),
            )
            .unwrap();
        }
    };

    rewrite(1);
    let u = e.run().record.unusual.expect("stopped");
    assert!(u.stopped && u.before_pinned && u.pinned_by_flux, "{u:?}");
    // The backups out of reach: the alarm is settled, the pin is said to be left.
    let s =
        flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, true, None).unwrap();
    assert!(s.marks_left);
    let s =
        flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, true, Some(&repo))
            .unwrap();
    assert!(!s.marks_left);
    let kept = repo.find(&first.id).unwrap();
    assert!(!kept.pinned, "Flux's pin is off");
    assert_eq!(
        kept.note.as_deref(),
        Some("before the move"),
        "the owner's note stays"
    );

    // Another alarm, dealt with this time: the next backup isn't looked at, and isn't
    // learned from.
    let made = e.run();
    assert!(made.outcome.is_ok(), "{:?}", made.outcome);
    rewrite(2);
    let u = e.run().record.unusual.expect("stopped again");
    flux_runner::settle_unusual(&e.paths, &e.id, &e.profile, &u.snapshot, false, None).unwrap();
    let after = e.run();
    assert!(after.outcome.is_ok() && after.record.unusual.is_none());
    let history =
        flux_runner::sentry_history(&e.paths, &e.id, runlog::recent_runs(&e.paths, &e.id, 50));
    assert!(
        history.iter().all(|r| r.run_id != after.record.run_id),
        "dealt with: nothing is learned, the unlooked-at backup included"
    );
    assert!(
        history.iter().any(|r| r.run_id == made.record.run_id),
        "the backup after \"this was me\" is"
    );
}

/// Flux Sentry's look before a Vault backup reads the snapshot's file list from the
/// cache the last backup left (the Vault gives it a folder at a time: hours over a
/// network). It must be the very list the Vault holds.
#[test]
fn the_look_reads_a_vaults_file_list_from_the_cache() {
    let mut e = env();
    e.profile.engine = flux_core::Engine::Vault;
    e.profile.vault = Some(flux_core::profile::VaultOptions {
        encryption: false,
        ..Default::default()
    });
    let docs = e.root.join("src/docs/deep/er");
    fs::create_dir_all(&docs).unwrap();
    for i in 0..40 {
        fs::write(docs.join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
    }
    assert!(e.run().outcome.is_ok());
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snap = repo.snapshots().unwrap().pop().unwrap();
    let list = |it: flux_runner::repo::Entries<'_>| -> Vec<flux_fs::entry::Entry> {
        it.collect::<Result<_, _>>().unwrap()
    };
    let cached = list(repo.entries_for_look(&e.paths, &snap).unwrap());
    assert_eq!(cached, list(repo.entries(&snap).unwrap()));
    assert!(cached.len() > 40);
    // And it is the cache that was read: without it, the list still comes (the slow way).
    let cache = e.paths.cache_dir.join("vault");
    assert!(cache.is_dir(), "the backup left its file list in the cache");
    fs::remove_dir_all(&cache).unwrap();
    assert_eq!(
        list(repo.entries_for_look(&e.paths, &snap).unwrap()),
        cached
    );
    // A changed file is seen against the cached list, and the next backup goes through.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(docs.join("f0.txt"), "edited").unwrap();
    let r = e.run();
    assert!(r.outcome.is_ok(), "{:?}", r.outcome);
    assert_eq!(r.record.sentry_seen.map(|s| s.touched), Some(1));
}
