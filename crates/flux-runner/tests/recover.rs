//! Recovering backups on a new computer, simulated: "the old computer" is a profile whose
//! origin names another computer and user, so its backups are kept exactly as that
//! computer's would be; "the new computer" is another Flux home with no profiles.

use std::fs;
use std::path::{Path, PathBuf};

use flux_core::profile::{Engine, Origin, PasswordSource, Profile, Target, Trigger as When};
use flux_core::progress::{Progress, silent};
use flux_core::{Paths, ProfileId, Scope};
use flux_runner::recover::{self, Then};
use flux_runner::{BackupJob, RunStatus, Trigger, run_backup};

fn run(paths: &Paths, id: &ProfileId, p: &Profile) -> RunStatus {
    let r = run_backup(
        &BackupJob {
            paths,
            id,
            profile: p,
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
    .record;
    if let Some(e) = &r.error {
        eprintln!("run failed: {e}");
    }
    r.status
}

/// The old computer's backup of `src` to `tgt`, made as oldpc/olduser/home.
fn old_profile(root: &Path, engine: Engine) -> Profile {
    let mut p = Profile::new(
        "Home folder",
        Target::Local {
            path: root.join("tgt"),
            fs_uuid: None,
        },
        vec![root.join("src")],
    );
    p.engine = engine;
    p.options.notify = false;
    p.sources.exclude = vec!["*.tmp".into()];
    p.hooks.pre_backup = vec!["true secret-token".into()];
    p.origin = Some(Origin {
        host: "oldpc".into(),
        user: "olduser".into(),
        profile: "home".into(),
    });
    p
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().to_owned();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tgt")).unwrap();
    fs::write(root.join("src/f"), "data").unwrap();
    (t, root)
}

fn snapshots(paths: &Paths, id: &ProfileId, p: &Profile) -> usize {
    flux_runner::repo::AnyRepo::open(paths, id, p, None)
        .unwrap()
        .snapshots()
        .unwrap()
        .len()
}

#[test]
fn a_mirror_backup_is_recovered_and_carried_on() {
    let (_t, root) = setup();
    let old = Paths::under(root.join("old"), Scope::User);
    let (id, p) = (
        ProfileId::new("home").unwrap(),
        old_profile(&root, Engine::Mirror),
    );
    assert_eq!(run(&old, &id, &p), RunStatus::Completed);

    // The settings are kept with the backups, without the hook's command.
    let folder = root.join("tgt/flux-backup/oldpc/olduser/home");
    let copy = fs::read_to_string(folder.join("flux-profile.toml")).unwrap();
    assert!(
        copy.contains("Home folder") && copy.contains("*.tmp"),
        "{copy}"
    );
    assert!(!copy.contains("secret-token"), "hooks stay behind");
    assert!(copy.contains("made-on = \"oldpc\""));
    assert_eq!(recover::copy_current(&old, &id, &p), Some(true));
    // Unchanged settings aren't written again; changed ones are, at the next backup.
    let before = fs::metadata(folder.join("flux-profile.toml"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(root.join("src/g"), "more").unwrap();
    assert_eq!(run(&old, &id, &p), RunStatus::Completed);
    let after = fs::metadata(folder.join("flux-profile.toml"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after);
    let mut changed = p.clone();
    changed.retention.keep_daily = 3;
    assert_eq!(recover::copy_current(&old, &id, &changed), Some(false));

    // The new computer: no profiles. The drive is picked (or its flux-backup folder).
    let new = Paths::under(root.join("new"), Scope::User);
    for picked in [root.join("tgt"), root.join("tgt/flux-backup")] {
        let target = Target::Local {
            path: picked,
            fs_uuid: None,
        };
        let s = recover::find(&new, &target, None, true).unwrap();
        assert_eq!(s.found.len(), 1, "{s:?}");
        let f = &s.found[0];
        assert_eq!(
            (f.engine, f.name.as_str(), f.saved),
            (Engine::Mirror, "Home folder", true)
        );
        assert_eq!(
            (f.kept.host.as_str(), f.kept.user.as_str()),
            ("oldpc", "olduser")
        );
        assert_eq!(f.snapshots, Some(2));
        assert!(!f.here);
    }
    let target = Target::Local {
        path: root.join("tgt"),
        fs_uuid: None,
    };
    let found = recover::find(&new, &target, None, true)
        .unwrap()
        .found
        .remove(0);
    let (nid, np) = recover::recover(&new, &found, &target, Then::CarryOn, None).unwrap();
    assert_eq!(nid.as_str(), "home");
    assert_eq!(np.sources.exclude, vec!["*.tmp".to_owned()]);
    assert!(np.hooks.pre_backup.is_empty());
    assert_eq!(np.origin.as_ref().unwrap().host, "oldpc");
    assert_eq!(snapshots(&new, &nid, &np), 2, "the old snapshots are there");
    // Carrying on adds to the same backups.
    fs::write(root.join("src/h"), "new computer").unwrap();
    assert_eq!(run(&new, &nid, &np), RunStatus::Completed);
    assert_eq!(snapshots(&new, &nid, &np), 3);
    assert_eq!(
        fs::read_dir(root.join("tgt/flux-backup")).unwrap().count(),
        1
    );
    // A second recovery gets an id of its own; "just restore" schedules nothing.
    let (id2, p2) = recover::recover(&new, &found, &target, Then::Restore, None).unwrap();
    assert_eq!(id2.as_str(), "home-2");
    assert_eq!(p2.schedule.trigger, When::Manual);
    assert_eq!(snapshots(&new, &id2, &p2), 3);
    assert!(recover::recover(&new, &found, &target, Then::Restore, Some("home")).is_err());
}

/// The settings kept with a backup are read as a description, never as instructions: on a
/// drive anyone can write to, they could have been given hooks, a password command or
/// ssh options that run a program on the computer recovering them.
#[test]
fn settings_found_on_a_drive_cannot_run_anything() {
    let (_t, root) = setup();
    let old = Paths::under(root.join("old"), Scope::User);
    let (id, p) = (
        ProfileId::new("home").unwrap(),
        old_profile(&root, Engine::Rsync),
    );
    if run(&old, &id, &p) != RunStatus::Completed {
        eprintln!("skipping: no rsync");
        return;
    }
    // Someone with the drive adds to the saved settings.
    let saved = root.join("tgt/flux-rsync/oldpc/olduser/home/flux-profile.toml");
    let pwned = root.join("pwned");
    let mut text = fs::read_to_string(&saved).unwrap();
    text.push_str(&format!(
        "\n[hooks]\npre_backup = [\"touch {p}\"]\npost_backup = [\"touch {p}\"]\n\n[rsync]\nextra_args = [\"--rsh=touch {p}\"]\n\n[[webhooks]]\nurl = \"http://127.0.0.1:9/hook\"\n",
        p = pwned.display()
    ));
    fs::write(&saved, text).unwrap();

    let new = Paths::under(root.join("new"), Scope::User);
    let target = Target::Local {
        path: root.join("tgt"),
        fs_uuid: None,
    };
    let s = recover::find(&new, &target, None, true).unwrap();
    assert_eq!(s.found.len(), 1, "{s:?}");
    let got = recover::recovered_profile(&s.found[0], &target, Then::CarryOn);
    assert_eq!(got.hooks, Default::default());
    assert!(got.webhooks.is_empty());
    assert!(got.rsync.is_none_or(|r| r.extra_args.is_empty()));
    assert_eq!(got.name, "Home folder", "the description itself is kept");
    assert_eq!(got.sources.exclude, vec!["*.tmp".to_string()]);

    // Recovered and carried on: a backup runs, and nothing else does.
    let (rid, rp) = recover::recover(&new, &s.found[0], &target, Then::CarryOn, None).unwrap();
    let mut rp = rp;
    rp.options.notify = false;
    fs::write(root.join("src/h"), "new computer").unwrap();
    assert_eq!(run(&new, &rid, &rp), RunStatus::Completed);
    assert!(!pwned.exists(), "a command from the drive ran");
}

#[test]
fn an_encrypted_vault_needs_its_password_to_be_found() {
    let (_t, root) = setup();
    let key = root.join("key");
    fs::write(&key, "s3cret\n").unwrap();
    let old = Paths::under(root.join("old"), Scope::User);
    let id = ProfileId::new("home").unwrap();
    let mut p = old_profile(&root, Engine::Vault);
    p.vault = Some(flux_core::profile::VaultOptions {
        password_source: PasswordSource::File { path: key.clone() },
        ..Default::default()
    });
    assert_eq!(run(&old, &id, &p), RunStatus::Completed);

    let new = Paths::under(root.join("new"), Scope::User);
    let target = Target::Local {
        path: root.join("tgt"),
        fs_uuid: None,
    };
    let s = recover::find(&new, &target, None, true).unwrap();
    assert!(s.vault_locked && !s.wrong_password && s.found.is_empty());
    let s = recover::find(&new, &target, Some("wrong"), true).unwrap();
    assert!(s.vault_locked && s.wrong_password);
    let s = recover::find(&new, &target, Some("s3cret"), true).unwrap();
    assert_eq!(s.found.len(), 1, "{s:?}");
    let f = &s.found[0];
    assert_eq!(
        (f.engine, f.saved, f.snapshots),
        (Engine::Vault, true, Some(1))
    );
    assert_eq!(f.sources, vec![root.join("src")]);
    assert_eq!(f.kept.profile, "home");

    let (nid, np) = recover::recover(&new, f, &target, Then::CarryOn, Some("laptop")).unwrap();
    assert_eq!(nid.as_str(), "laptop");
    // The password comes from the same file here.
    assert_eq!(snapshots(&new, &nid, &np), 1);
    fs::write(root.join("src/g"), "more").unwrap();
    assert_eq!(run(&new, &nid, &np), RunStatus::Completed);
    assert_eq!(
        snapshots(&new, &nid, &np),
        2,
        "carried on as the old computer's backup"
    );
}

#[test]
fn backups_without_saved_settings_are_found_too() {
    let (_t, root) = setup();
    let old = Paths::under(root.join("old"), Scope::User);
    let id = ProfileId::new("home").unwrap();
    let mut p = old_profile(&root, Engine::Rsync);
    p.rsync = Some(Default::default());
    if flux_compat::tools::require_rsync().is_err() {
        return; // rsync isn't installed
    }
    assert_eq!(run(&old, &id, &p), RunStatus::Completed);
    let folder = root.join("tgt/flux-rsync/oldpc/olduser/home");
    // A backup made before 1.0: no settings kept.
    fs::remove_file(folder.join("flux-profile.toml")).unwrap();
    let new = Paths::under(root.join("new"), Scope::User);
    let target = Target::Local {
        path: root.join("tgt"),
        fs_uuid: None,
    };
    let f = recover::find(&new, &target, None, true)
        .unwrap()
        .found
        .remove(0);
    assert_eq!(
        (f.engine, f.saved, f.name.as_str()),
        (Engine::Rsync, false, "home")
    );
    assert_eq!(f.snapshots, Some(1));
    // The profile reaches the backups; its folders are the newest snapshot's.
    let (nid, np) = recover::recover(&new, &f, &target, Then::Restore, None).unwrap();
    assert_eq!(np.engine, Engine::Rsync);
    assert_eq!(np.sources.paths, vec![root.join("src")]);
    assert_eq!(snapshots(&new, &nid, &np), 1);
}

#[test]
fn readiness_says_what_a_recovery_would_need() {
    let (_t, root) = setup();
    let key = root.join("key");
    fs::write(&key, "s3cret\n").unwrap();
    let paths = Paths::under(root.join("home"), Scope::User);
    let id = ProfileId::new("home").unwrap();
    let mut p = old_profile(&root, Engine::Vault);
    p.origin = None;
    p.vault = Some(flux_core::profile::VaultOptions {
        password_source: PasswordSource::File { path: key },
        ..Default::default()
    });
    let ok = |c: &[recover::Check], k: &str| c.iter().find(|c| c.key == k).map(|c| c.ok);
    let c = recover::readiness(&paths, &id, &p);
    assert_eq!(
        c.iter().map(|c| c.key).collect::<Vec<_>>(),
        ["settings", "password_elsewhere", "restore_test", "offsite"]
    );
    assert!(c.iter().all(|c| !c.ok));
    assert_eq!(run(&paths, &id, &p), RunStatus::Completed);
    let c = recover::readiness(&paths, &id, &p);
    assert_eq!(ok(&c, "settings"), Some(true));
    assert!(c[0].detail.contains("in the Vault"));
    recover::set_answer(&paths, &id, "password_elsewhere", true).unwrap();
    assert!(recover::set_answer(&paths, &id, "nonsense", true).is_err());
    p.offsite = true;
    let c = recover::readiness(&paths, &id, &p);
    assert_eq!(ok(&c, "password_elsewhere"), Some(true));
    assert_eq!(ok(&c, "offsite"), Some(true));
    // Changed settings: the copy is behind until the next backup.
    assert_eq!(ok(&c, "settings"), Some(false));
    // A plain Mirror has no password to keep.
    let mut m = old_profile(&root, Engine::Mirror);
    m.origin = None;
    assert!(ok(&recover::readiness(&paths, &id, &m), "password_elsewhere").is_none());
    // Borg with encryption asks about its key.
    let mut b = m.clone();
    b.engine = Engine::Borg;
    b.borg = Some(Default::default());
    let c = recover::readiness(&paths, &id, &b);
    assert!(
        c.iter()
            .any(|c| c.key == "key_exported" && c.detail.contains("borg key export"))
    );
}

#[test]
fn the_recovery_tool_is_kept_next_to_a_vault_on_a_drive() {
    let (_t, root) = setup();
    let tool = root.join("flux-vault-recover-build");
    fs::write(&tool, "#!/bin/sh\necho reader v1\n").unwrap();
    let mut p = old_profile(&root, Engine::Vault);
    p.vault = Some(Default::default());
    assert!(recover::keep_tool(&p, Some(&tool)).unwrap());
    let kept = root.join("tgt/flux-vault-recover");
    assert_eq!(fs::read(&kept).unwrap(), fs::read(&tool).unwrap());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let note = fs::read_to_string(root.join("tgt/FLUX-VAULT-RECOVERY.txt")).unwrap();
    assert!(
        note.contains("./flux-vault-recover flux-vault restore latest"),
        "{note}"
    );
    // Already there: left alone; a newer tool replaces it.
    assert!(!recover::keep_tool(&p, Some(&tool)).unwrap());
    fs::write(&tool, "#!/bin/sh\necho reader v2\n").unwrap();
    assert!(recover::keep_tool(&p, Some(&tool)).unwrap());
    assert_eq!(fs::read(&kept).unwrap(), fs::read(&tool).unwrap());
    // Not without a tool, when turned off, or for other kinds of backup.
    assert!(!recover::keep_tool(&p, None).unwrap());
    p.vault.as_mut().unwrap().keep_recovery_tool = false;
    assert!(!recover::keep_tool(&p, Some(&tool)).unwrap());
    let m = old_profile(&root, Engine::Mirror);
    assert!(!recover::keep_tool(&m, Some(&tool)).unwrap());
    // The setting is written only when off.
    let text = p.to_toml().unwrap();
    assert!(text.contains("keep_recovery_tool = false"), "{text}");
    p.vault.as_mut().unwrap().keep_recovery_tool = true;
    assert!(!p.to_toml().unwrap().contains("keep_recovery_tool"));
}
