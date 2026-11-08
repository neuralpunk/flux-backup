//! End-to-end tests of the `flux` binary, isolated with `FLUX_HOME`.

use std::path::Path;
use std::process::{Command, Output};

fn flux(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(args)
        .env("FLUX_HOME", home)
        // Never touch the real ~/.config/borg and ~/.cache/borg.
        .env("BORG_BASE_DIR", home.join("borg-base"))
        // Never touch the real systemd configuration from tests.
        .env("FLUX_SYSTEMD_UNIT_DIR", home.join("systemd-units"))
        .env("FLUX_SYSTEMCTL", "true")
        .env_remove("FLUX_LOG")
        .output()
        .expect("run flux")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn profile_lifecycle() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();

    let o = flux(h, &["profile", "list"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("No profiles yet"));

    let o = flux(
        h,
        &[
            "profile",
            "create",
            "home",
            "--target",
            "/mnt/backup",
            "-s",
            "/home/me",
            "-x",
            "*.tmp",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(h.join("config/profiles/home.toml").is_file());

    let o = flux(
        h,
        &[
            "profile",
            "create",
            "docs",
            "--target",
            "me@nas:/srv/b",
            "-s",
            "/d",
            "--engine",
            "vault",
        ],
    );
    assert!(o.status.success());

    // Duplicate id fails.
    let o = flux(
        h,
        &["profile", "create", "home", "--target", "/x", "-s", "/y"],
    );
    assert_eq!(o.status.code(), Some(1));

    let o = flux(h, &["--json", "profile", "list"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["id"], "docs");
    assert_eq!(v[0]["profile"]["engine"], "vault");
    assert_eq!(v[0]["profile"]["target"]["host"], "nas");
    assert_eq!(v[1]["profile"]["sources"]["exclude"][0], "*.tmp");

    let o = flux(h, &["profile", "validate"]);
    assert!(o.status.success(), "{}", stdout(&o));

    // Break a profile by hand: validate reports it with exit code 3.
    let f = h.join("config/profiles/home.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("paths = [\"/home/me\"]", "paths = []");
    std::fs::write(&f, text).unwrap();
    let o = flux(h, &["profile", "validate", "home"]);
    assert_eq!(o.status.code(), Some(3));
    assert!(stdout(&o).contains("sources.paths"));

    // Delete without a terminal and without --yes is refused.
    let o = flux(h, &["profile", "delete", "home"]);
    assert!(!o.status.success());
    let o = flux(h, &["profile", "delete", "home", "--yes"]);
    assert!(o.status.success());
    assert!(!f.exists());
}

#[test]
fn invalid_input_is_rejected() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let o = flux(
        h,
        &["profile", "create", "Bad Id", "--target", "/x", "-s", "/y"],
    );
    assert_eq!(o.status.code(), Some(1));
    let o = flux(
        h,
        &["profile", "create", "a", "--target", "relative", "-s", "/y"],
    );
    assert_eq!(o.status.code(), Some(1));
    let o = flux(h, &["profile", "show", "missing"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not found"));
}

#[test]
fn config_path_respects_flux_home() {
    let home = tempfile::tempdir().unwrap();
    let o = flux(home.path(), &["--json", "config", "path"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v["profiles_dir"],
        home.path().join("config/profiles").to_str().unwrap()
    );
    assert_eq!(v["scope"], "user");
}

#[test]
fn backup_browse_restore() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    let tgt = h.join("tgt");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::create_dir_all(&tgt).unwrap();
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    let (s, t) = (src.to_str().unwrap(), tgt.to_str().unwrap());
    assert!(
        flux(h, &["profile", "create", "p", "-t", t, "-s", s])
            .status
            .success()
    );

    let o = flux(h, &["--json", "backup", "p", "--dry-run"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["result"]["outcome"], "dry_run");
    assert_eq!(v["result"]["stats"]["copy_files"], 1);

    let o = flux(h, &["--json", "backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["result"]["outcome"], "created");
    assert_eq!(v["run"]["status"], "completed");
    let id = v["result"]["snapshot"]["id"].as_str().unwrap().to_owned();

    let o = flux(h, &["backup", "p"]);
    assert!(stdout(&o).contains("Nothing changed"));

    let o = flux(h, &["ls", "p", s, "-R"]);
    assert_eq!(stdout(&o), "docs/\ndocs/a.txt\n");

    let o = flux(h, &["--json", "snapshots", "p"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v[0]["id"], id.as_str());

    std::fs::remove_file(src.join("docs/a.txt")).unwrap();
    // In-place restore needs confirmation; without a terminal it is refused.
    assert!(
        !flux(h, &["restore", "p", src.join("docs").to_str().unwrap()])
            .status
            .success()
    );
    let o = flux(
        h,
        &["restore", "p", src.join("docs").to_str().unwrap(), "-y"],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        std::fs::read_to_string(src.join("docs/a.txt")).unwrap(),
        "alpha"
    );

    let out = h.join("out");
    let o = flux(h, &["restore", "p", s, "--to", out.to_str().unwrap()]);
    assert!(o.status.success());
    assert_eq!(
        std::fs::read_to_string(out.join("src/docs/a.txt")).unwrap(),
        "alpha"
    );
}

#[test]
fn lifecycle_commands() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::write(src.join("a.txt"), "1").unwrap();
    let s = src.to_str().unwrap();
    assert!(
        flux(
            h,
            &[
                "profile",
                "create",
                "p",
                "-t",
                h.join("tgt").to_str().unwrap(),
                "-s",
                s
            ]
        )
        .status
        .success()
    );
    // Keep only the newest snapshot (plus pinned/named), no notifications.
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("keep_within = \"1day\"\n", "")
        .replace("keep_daily = 7", "keep_daily = 0")
        .replace("keep_weekly = 4", "keep_weekly = 0")
        .replace("keep_monthly = 12", "keep_monthly = 0")
        .replace("keep_yearly = 5", "keep_yearly = 0")
        .replace("notify = true", "notify = false");
    std::fs::write(&f, text).unwrap();

    let snap = |h: &Path| -> String {
        let o = flux(h, &["--json", "backup", "p", "--no-prune"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v["result"]["snapshot"]["id"].as_str().unwrap().to_owned()
    };
    let s1 = snap(h);
    std::fs::write(src.join("b.txt"), "2").unwrap();
    let s2 = snap(h);
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(src.join("a.txt"), "11").unwrap();
    let s3 = snap(h);

    assert!(flux(h, &["pin", "p", &s1]).status.success());
    assert!(flux(h, &["name", "p", &s2, "milestone"]).status.success());
    // Duplicate names are refused.
    assert!(!flux(h, &["name", "p", &s3, "milestone"]).status.success());

    let o = flux(h, &["--json", "diff", "p"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        (v["old"].as_str().unwrap(), v["new"].as_str().unwrap()),
        (s2.as_str(), s3.as_str())
    );
    assert_eq!(v["summary"]["modified"], 1);

    let o = flux(h, &["--json", "find", "p", "*.TXT", "-i"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);

    let o = flux(
        h,
        &[
            "--json",
            "history",
            "p",
            src.join("a.txt").to_str().unwrap(),
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2, "a.txt has two versions");

    // Nothing to prune: s1 pinned, s2 named, s3 newest.
    let o = flux(h, &["--json", "prune", "p", "--dry-run"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["removed"].as_array().unwrap().len(), 0);
    assert!(flux(h, &["unpin", "p", &s1]).status.success());
    let o = flux(h, &["--json", "prune", "p"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["removed"][0], s1.as_str());

    // Delete refuses pinned without --force; works with --yes.
    assert!(flux(h, &["pin", "p", &s2]).status.success());
    assert!(!flux(h, &["delete", "p", &s2, "-y"]).status.success());
    assert!(
        flux(h, &["delete", "p", &s2, "-y", "--force"])
            .status
            .success()
    );

    let o = flux(h, &["--json", "log", "p"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
    assert_eq!(v[0]["snapshot"], s3.as_str());
    assert_eq!(v[0]["status"], "completed");
}

#[test]
fn schedule_install_sync_remove() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let units = h.join("units");
    let calls = h.join("systemctl.log");
    let fake = h.join("fake-systemctl");
    write_executable(
        &fake,
        &format!("#!/bin/sh\necho \"$@\" >> {}\n", calls.display()),
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .args(args)
            .env("FLUX_HOME", h)
            // Never touch the real ~/.config/borg and ~/.cache/borg.
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SYSTEMD_UNIT_DIR", &units)
            .env("FLUX_SYSTEMCTL", &fake)
            .output()
            .unwrap()
    };
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    assert!(
        run(&[
            "profile",
            "create",
            "p",
            "-t",
            h.join("tgt").to_str().unwrap(),
            "-s",
            h.join("src").to_str().unwrap()
        ])
        .status
        .success()
    );

    // Manual profile: nothing to install.
    let o = run(&["schedule", "install", "p"]);
    assert!(stdout(&o).contains("nothing to schedule"));
    assert!(!units.join("flux-backup-p.timer").exists());

    // Make it an interval schedule and sync.
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f).unwrap().replace(
        "trigger = \"manual\"",
        "trigger = \"interval\"\nevery = \"6h\"",
    );
    std::fs::write(&f, text).unwrap();
    let o = run(&["schedule", "sync"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let service = std::fs::read_to_string(units.join("flux-backup-p.service")).unwrap();
    assert!(service.contains("backup p --scheduled --if-due"));
    assert!(service.contains("FLUX_HOME="));
    assert!(
        std::fs::read_to_string(units.join("flux-backup-p.timer"))
            .unwrap()
            .contains("OnUnitActiveSec=900s")
    );
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(log.contains("--user daemon-reload"));
    assert!(log.contains("--user enable --now flux-backup-p.timer"));

    // A second sync changes nothing.
    std::fs::remove_file(&calls).unwrap();
    let o = run(&["schedule", "sync"]);
    assert!(stdout(&o).contains("up to date"));
    assert!(
        !std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .contains("daemon-reload")
    );

    // A foreign unit with our name is never overwritten.
    std::fs::write(
        units.join("flux-backup-p.timer"),
        "[Timer]\nOnCalendar=hourly\n",
    )
    .unwrap();
    let o = run(&["schedule", "install", "p"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("not generated by Flux"));
    std::fs::remove_file(units.join("flux-backup-p.timer")).unwrap();
    assert!(run(&["schedule", "install", "p"]).status.success());

    // Deleting the profile removes its units.
    assert!(run(&["profile", "delete", "p", "--yes"]).status.success());
    assert!(!units.join("flux-backup-p.service").exists());
    assert!(!units.join("flux-backup-p.timer").exists());
    assert!(
        std::fs::read_to_string(&calls)
            .unwrap()
            .contains("disable --now flux-backup-p.timer")
    );
}

#[test]
fn status_watch_cancel_without_jobs() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let o = flux(h, &["status"]);
    assert!(stdout(&o).contains("No profiles yet"));
    std::fs::create_dir_all(h.join("s")).unwrap();
    assert!(
        flux(
            h,
            &[
                "profile",
                "create",
                "p",
                "-t",
                h.to_str().unwrap(),
                "-s",
                h.join("s").to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let o = flux(h, &["status"]);
    assert!(stdout(&o).contains("idle"));
    let o = flux(h, &["watch", "p"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no backup of p is running"));
    assert!(!flux(h, &["cancel", "p"]).status.success());
}

fn flux_pw(home: &Path, pw: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(args)
        .env("FLUX_HOME", home)
        // Never touch the real ~/.config/borg and ~/.cache/borg.
        .env("BORG_BASE_DIR", home.join("borg-base"))
        .env("FLUX_PASSWORD", pw)
        .env("FLUX_SYSTEMD_UNIT_DIR", home.join("systemd-units"))
        .env("FLUX_SYSTEMCTL", "true")
        .env("FLUX_SECRET_TOOL", "false")
        .output()
        .expect("run flux")
}

#[test]
fn vault_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    std::fs::write(src.join("docs/b.txt"), "alpha").unwrap(); // same content
    let s = src.to_str().unwrap();
    let pw = "s3cret pass";
    assert!(
        flux(
            h,
            &[
                "profile",
                "create",
                "v",
                "-t",
                h.join("tgt").to_str().unwrap(),
                "-s",
                s,
                "--engine",
                "vault"
            ]
        )
        .status
        .success()
    );
    let f = h.join("config/profiles/v.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false");
    std::fs::write(&f, text).unwrap();

    // No password available and no terminal: a clear error, nothing created.
    let o = flux(h, &["backup", "v"]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("no password available"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    let o = flux_pw(h, pw, &["--json", "backup", "v"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let id1 = v["result"]["snapshot"]["id"].as_str().unwrap().to_owned();
    assert!(h.join("tgt/flux-vault/config").exists());

    // Wrong password is refused.
    let o = flux_pw(h, "wrong", &["snapshots", "v"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("wrong password"));

    assert!(stdout(&flux_pw(h, pw, &["backup", "v"])).contains("Nothing changed"));
    std::fs::write(src.join("docs/c.txt"), "gamma").unwrap();
    assert!(flux_pw(h, pw, &["backup", "v"]).status.success());

    let o = flux_pw(h, pw, &["ls", "v", src.join("docs").to_str().unwrap()]);
    assert_eq!(stdout(&o), "a.txt\nb.txt\nc.txt\n");
    let o = flux_pw(h, pw, &["--json", "diff", "v"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["summary"]["added"], 1);

    let out = h.join("out");
    let o = flux_pw(
        h,
        pw,
        &["restore", "v", s, "--to", out.to_str().unwrap(), "-s", &id1],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        std::fs::read_to_string(out.join("src/docs/b.txt")).unwrap(),
        "alpha"
    );
    assert!(
        !out.join("src/docs/c.txt").exists(),
        "restored the first snapshot"
    );

    let o = flux_pw(h, pw, &["check", "v", "--read-data"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("0 problems"));

    let o = flux_pw(h, pw, &["--json", "key", "list", "v"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["current"], true);

    let o = flux_pw(h, pw, &["--json", "prune", "v"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = flux_pw(h, pw, &["--json", "find", "v", "*.txt"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
}

/// Both engines against an "SSH server" that is a local stand-in for ssh: the real
/// OpenSSH sftp-server and rsync run, only the network hop is skipped.
#[test]
fn ssh_targets_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let Some(fake) = flux_transport::testing::fake_ssh(h) else {
        eprintln!("skipping: no sftp-server installed");
        return;
    };
    if Command::new("rsync").arg("--version").output().is_err() {
        eprintln!("skipping: no rsync");
        return;
    }
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .args(args)
            .env("FLUX_HOME", h)
            // Never touch the real ~/.config/borg and ~/.cache/borg.
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SSH", &fake)
            .env("FLUX_PASSWORD", "pw")
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true")
            .output()
            .unwrap()
    };
    let src = h.join("src");
    std::fs::create_dir_all(src.join("d")).unwrap();
    std::fs::write(src.join("d/a.txt"), "alpha").unwrap();
    std::fs::write(src.join("big"), vec![7u8; 300_000]).unwrap();
    let s = src.to_str().unwrap();
    for (id, engine) in [("m", "mirror"), ("v", "vault"), ("r", "rsync")] {
        let target = format!("localhost:{}", h.join(format!("remote-{id}")).display());
        assert!(
            run(&[
                "profile", "create", id, "-t", &target, "-s", s, "--engine", engine
            ])
            .status
            .success()
        );
        let f = h.join(format!("config/profiles/{id}.toml"));
        let text = std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false");
        std::fs::write(&f, text).unwrap();
    }

    for id in ["m", "v", "r"] {
        let o = run(&["--json", "backup", id]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            stdout(&run(&["backup", id])).contains("Nothing changed"),
            "{id}"
        );
    }
    std::fs::write(src.join("d/b.txt"), "beta").unwrap();
    for id in ["m", "v", "r"] {
        let o = run(&["backup", id]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = run(&["--json", "snapshots", id]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2, "{id}");
        let o = run(&["ls", id, src.join("d").to_str().unwrap()]);
        assert_eq!(stdout(&o), "a.txt\nb.txt\n", "{id}");
        let out = h.join(format!("out-{id}"));
        let o = run(&["restore", id, s, "--to", out.to_str().unwrap()]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(
            std::fs::read(out.join("src/big")).unwrap(),
            vec![7u8; 300_000]
        );
        assert_eq!(
            std::fs::read_to_string(out.join("src/d/b.txt")).unwrap(),
            "beta"
        );
        let o = run(&["check", id]);
        assert!(o.status.success(), "{id}: {}", stdout(&o));
    }

    // Mirror over SSH hard-links unchanged files on the server.
    let snaps: Vec<_> = glob_dirs(&h.join("remote-m/flux-backup"), "snapshots");
    assert_eq!(snaps.len(), 2);
    let ino = |d: &Path| {
        std::os::unix::fs::MetadataExt::ino(
            &std::fs::metadata(
                d.join("data")
                    .join(src.strip_prefix("/").unwrap())
                    .join("big"),
            )
            .unwrap(),
        )
    };
    assert_eq!(ino(&snaps[0]), ino(&snaps[1]));
    // So does rsync compatibility mode, and it keeps its recovery kit on the server.
    let snaps: Vec<_> = glob_dirs(&h.join("remote-r/flux-rsync"), "snapshots");
    assert_eq!(snaps.len(), 2);
    assert_eq!(ino(&snaps[0]), ino(&snaps[1]));
    let repo = snaps[0].parent().unwrap().parent().unwrap();
    let guide = std::fs::read_to_string(repo.join("FLUX-RECOVERY.txt")).unwrap();
    assert!(guide.contains("localhost:"), "{guide}");
    assert!(snaps[1].join("rsync-command.txt").exists());
    // The kit's script works from this computer with plain ssh and rsync.
    let bin = h.join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(&fake, bin.join("ssh")).unwrap();
    let script = h.join("flux-backup.sh");
    std::fs::copy(repo.join("flux-backup.sh"), &script).unwrap();
    std::fs::write(src.join("d/c.txt"), "gamma").unwrap();
    let o = Command::new("sh")
        .arg(&script)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/local/bin:/usr/bin:/bin", bin.display()),
        )
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let snaps: Vec<_> = glob_dirs(&h.join("remote-r/flux-rsync"), "snapshots");
    assert_eq!(snaps.len(), 3);
    assert_eq!(ino(&snaps[0]), ino(snaps.iter().max().unwrap()));
    let o = run(&["ls", "r", src.join("d").to_str().unwrap()]);
    assert_eq!(stdout(&o), "a.txt\nb.txt\nc.txt\n");

    // Prune and delete over SSH.
    let o = run(&["--json", "snapshots", "m"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let first = v[0]["id"].as_str().unwrap().to_owned();
    assert!(run(&["delete", "m", &first, "-y"]).status.success());
    assert_eq!(
        glob_dirs(&h.join("remote-m/flux-backup"), "snapshots").len(),
        1
    );
    assert!(run(&["prune", "v"]).status.success());
    let o = run(&["--json", "snapshots", "r"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let first = v[0]["id"].as_str().unwrap().to_owned();
    assert!(run(&["pin", "r", &first]).status.success());
    assert!(stdout(&run(&["snapshots", "r"])).contains("(pinned)"));
    assert!(run(&["unpin", "r", &first]).status.success());
    assert!(run(&["delete", "r", &first, "-y"]).status.success());
    assert_eq!(
        glob_dirs(&h.join("remote-r/flux-rsync"), "snapshots").len(),
        2
    );
}

#[test]
fn rsync_compatibility_mode() {
    if Command::new("rsync").arg("--version").output().is_err() {
        eprintln!("skipping: no rsync");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    for d in ["docs/sub", "cache", "keep"] {
        std::fs::create_dir_all(src.join(d)).unwrap();
    }
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    std::fs::hard_link(src.join("docs/a.txt"), src.join("docs/a-link.txt")).unwrap();
    std::fs::write(src.join("docs/x.tmp"), "temporary").unwrap();
    std::fs::write(src.join("keep/important.tmp"), "kept by !pattern").unwrap();
    std::fs::write(src.join("docs/sub/we ird [1]*.txt"), "odd name").unwrap();
    std::fs::write(
        src.join("cache/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::fs::write(src.join("cache/junk"), "cached").unwrap();
    std::os::unix::fs::symlink("a.txt", src.join("docs/link")).unwrap();
    let s = src.to_str().unwrap();
    let target = h.join("drive");
    std::fs::create_dir(&target).unwrap();
    let t = target.to_str().unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "plain",
            "-t",
            t,
            "-s",
            s,
            "-x",
            "*.tmp",
            "-x",
            "!important.tmp",
            "--engine",
            "rsync",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let f = h.join("config/profiles/plain.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false");
    std::fs::write(&f, text).unwrap();

    let o = flux(h, &["backup", "plain"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&flux(h, &["backup", "plain"])).contains("Nothing changed"));
    std::fs::write(src.join("docs/b.txt"), "beta").unwrap();
    assert!(
        flux(h, &["backup", "plain", "--name", "second"])
            .status
            .success()
    );

    // Plain folders written by rsync: hard links between snapshots and within one.
    let snaps = glob_dirs(&target.join("flux-rsync"), "snapshots");
    assert_eq!(snaps.len(), 2);
    let repo = snaps[0].parent().unwrap().parent().unwrap().to_owned();
    let data = |snap: &Path, rel: &str| {
        snap.join("data")
            .join(src.strip_prefix("/").unwrap())
            .join(rel)
    };
    let ino = |p: std::path::PathBuf| {
        std::os::unix::fs::MetadataExt::ino(&std::fs::symlink_metadata(p).unwrap())
    };
    assert_eq!(
        ino(data(&snaps[0], "docs/a.txt")),
        ino(data(&snaps[1], "docs/a.txt"))
    );
    assert_eq!(
        ino(data(&snaps[1], "docs/a.txt")),
        ino(data(&snaps[1], "docs/a-link.txt"))
    );
    assert!(!data(&snaps[1], "docs/x.tmp").exists());
    assert!(data(&snaps[1], "keep/important.tmp").exists());
    assert!(data(&snaps[1], "cache").is_dir());
    assert!(!data(&snaps[1], "cache/junk").exists());
    assert!(data(&snaps[1], "docs/sub/we ird [1]*.txt").exists());
    let command = std::fs::read_to_string(snaps[1].join("rsync-command.txt")).unwrap();
    assert!(command.contains("--link-dest=../../"), "{command}");
    assert!(snaps[1].join("rsync-filter.txt").exists());
    assert!(repo.join("latest").is_symlink());

    // Every Flux feature reads the plain folders.
    let o = flux(h, &["ls", "plain", src.join("docs").to_str().unwrap()]);
    assert_eq!(
        stdout(&o),
        "a-link.txt\na.txt\nb.txt\nlink -> a.txt\nsub/\n",
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(stdout(&flux(h, &["diff", "plain"])).contains("1 added"));
    let out = h.join("restored");
    let o = flux(h, &["restore", "plain", s, "--to", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        std::fs::read_to_string(out.join("src/docs/b.txt")).unwrap(),
        "beta"
    );
    assert_eq!(
        std::fs::read_link(out.join("src/docs/link")).unwrap(),
        Path::new("a.txt")
    );
    let o = flux(h, &["check", "plain", "--read-data"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("0 problems"), "{}", stdout(&o));

    // The recovery kit: printed by `flux recovery` and kept next to the backups.
    let o = flux(h, &["recovery", "plain"]);
    assert!(o.status.success());
    assert_eq!(
        stdout(&o).lines().next(),
        std::fs::read_to_string(repo.join("FLUX-RECOVERY.txt"))
            .unwrap()
            .lines()
            .next()
    );
    assert!(stdout(&flux(h, &["recovery", "plain", "--script"])).starts_with("#!/bin/sh"));

    // The script makes a snapshot with rsync alone, without Flux in the environment …
    std::fs::write(src.join("docs/c.txt"), "gamma").unwrap();
    let o = Command::new("sh")
        .arg(repo.join("flux-backup.sh"))
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let snaps = glob_dirs(&target.join("flux-rsync"), "snapshots");
    assert_eq!(snaps.len(), 3);
    let newest = snaps.iter().max().unwrap();
    assert_eq!(
        ino(data(&snaps[0], "docs/a.txt")),
        ino(data(newest, "docs/a.txt"))
    );
    assert!(data(newest, "docs/c.txt").exists());
    assert!(!data(newest, "cache/junk").exists());
    assert!(!data(newest, "docs/x.tmp").exists());

    // … which Flux then lists, browses and restores like its own.
    let o = flux(h, &["--json", "snapshots", "plain"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
    let o = flux(h, &["ls", "plain", src.join("docs").to_str().unwrap()]);
    assert!(stdout(&o).contains("c.txt"));

    // Pins and names survive retention; delete removes the folder.
    let first = v[0]["id"].as_str().unwrap().to_owned();
    assert!(flux(h, &["pin", "plain", &first]).status.success());
    let o = flux(h, &["delete", "plain", "second", "-y"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(glob_dirs(&target.join("flux-rsync"), "snapshots").len(), 2);
    assert!(flux(h, &["prune", "plain"]).status.success());
}

#[test]
fn export_and_import_profiles() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    std::fs::write(src.join("docs/x.tmp"), "temp").unwrap();
    let drive = h.join("drive");
    std::fs::create_dir(&drive).unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "home",
            "-t",
            drive.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "-x",
            "*.tmp",
        ],
    );
    assert!(o.status.success());

    // Every format exports; the notes go to stderr.
    let out = h.join("out");
    std::fs::create_dir(&out).unwrap();
    for f in ["flux", "borgmatic", "restic", "rsync", "borg"] {
        let o = flux(
            h,
            &[
                "profile",
                "export",
                "home",
                "-f",
                f,
                "-o",
                out.to_str().unwrap(),
            ],
        );
        assert!(
            o.status.success(),
            "{f}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let names: std::collections::BTreeSet<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for n in [
        "home.flux-profile.toml",
        "home-borgmatic.yaml",
        "home-restic-backup.sh",
        "home-rsync-backup.sh",
        "home-borg-backup.sh",
    ] {
        assert!(names.contains(n), "{n} in {names:?}");
    }
    assert!(
        !flux(h, &["profile", "export", "home", "-f", "nope"])
            .status
            .success()
    );

    // The Flux export imports again; an existing id isn't replaced.
    let file = out.join("home.flux-profile.toml");
    let o = flux(h, &["profile", "import", file.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("exists already"));
    let o = flux(
        h,
        &["profile", "import", file.to_str().unwrap(), "--id", "copy"],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let show = |id: &str| {
        let v: serde_json::Value =
            serde_json::from_str(&stdout(&flux(h, &["--json", "profile", "show", id]))).unwrap();
        v["profile"].clone()
    };
    assert_eq!(show("home"), show("copy"));

    // The rsync script backs up without Flux.
    if Command::new("rsync").arg("--version").output().is_ok() {
        let o = Command::new("sh")
            .arg(out.join("home-rsync-backup.sh"))
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let snaps = glob_dirs(&drive.join("flux-rsync"), "snapshots");
        assert_eq!(snaps.len(), 1);
        let data = snaps[0].join("data").join(src.strip_prefix("/").unwrap());
        assert!(data.join("docs/a.txt").exists());
        assert!(!data.join("docs/x.tmp").exists());
    }
}

fn borg_available() -> bool {
    Command::new("borg")
        .arg("--version")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("borg 1."))
}

#[test]
fn borg_compatibility_mode() {
    if !borg_available() {
        eprintln!("skipping: Borg 1.x is not installed");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let fake = flux_transport::testing::fake_ssh(h);
    let run = |pw: &str, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flux"));
        c.args(args)
            .env("FLUX_HOME", h)
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_PASSWORD", pw)
            .env("FLUX_SECRET_TOOL", "false")
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true");
        if let Some(f) = &fake {
            c.env("FLUX_SSH", f);
        }
        c.output().unwrap()
    };
    let src = h.join("src");
    for d in ["docs", "cache", "keep"] {
        std::fs::create_dir_all(src.join(d)).unwrap();
    }
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    std::fs::hard_link(src.join("docs/a.txt"), src.join("docs/a-link.txt")).unwrap();
    std::fs::write(src.join("docs/x.tmp"), "temporary").unwrap();
    std::fs::write(src.join("keep/important.tmp"), "kept").unwrap();
    std::fs::write(
        src.join("cache/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::fs::write(src.join("cache/junk"), "cached").unwrap();
    std::os::unix::fs::symlink("a.txt", src.join("docs/link")).unwrap();
    let s = src.to_str().unwrap();
    let drive = h.join("drive");
    std::fs::create_dir(&drive).unwrap();
    let mut ids = vec![("local", drive.to_str().unwrap().to_owned())];
    if fake.is_some() {
        ids.push((
            "remote",
            format!("localhost:{}", h.join("server").display()),
        ));
    }
    for (id, target) in &ids {
        let o = run(
            "pw",
            &[
                "profile",
                "create",
                id,
                "-t",
                target,
                "-s",
                s,
                "-x",
                "*.tmp",
                "-x",
                "!important.tmp",
                "--engine",
                "borg",
            ],
        );
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let f = h.join(format!("config/profiles/{id}.toml"));
        let text = std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false");
        std::fs::write(&f, text).unwrap();
    }

    for (id, _) in &ids {
        let _ = std::fs::remove_file(src.join("docs/b.txt"));
        let o = run("pw", &["backup", id]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        std::fs::write(src.join("docs/b.txt"), format!("beta {id}")).unwrap();
        let o = run("pw", &["backup", id, "--name", "second"]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = run("pw", &["--json", "snapshots", id]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2, "{id}");
        assert_eq!(v[1]["name"], "second", "{id}");

        let o = run("pw", &["ls", id, src.join("docs").to_str().unwrap()]);
        assert_eq!(
            stdout(&o),
            "a-link.txt\na.txt\nb.txt\nlink -> a.txt\n",
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = run("pw", &["ls", id, src.join("keep").to_str().unwrap()]);
        assert_eq!(stdout(&o), "important.tmp\n", "{id}");
        let o = run("pw", &["ls", id, src.join("cache").to_str().unwrap()]);
        assert!(!stdout(&o).contains("junk"), "{id}: {}", stdout(&o));
        let d = run("pw", &["diff", id]);
        assert!(
            stdout(&d).contains("1 added"),
            "{id}: {}{}\n{}",
            stdout(&d),
            String::from_utf8_lossy(&d.stderr),
            stdout(&run("pw", &["--json", "snapshots", id]))
        );

        let out = h.join(format!("restored-{id}"));
        let o = run("pw", &["restore", id, s, "--to", out.to_str().unwrap()]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(out.join("src/docs/b.txt")).unwrap(),
            format!("beta {id}")
        );
        let ino = |p: std::path::PathBuf| {
            std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(p).unwrap())
        };
        assert_eq!(
            ino(out.join("src/docs/a.txt")),
            ino(out.join("src/docs/a-link.txt"))
        );
        // The staging folder is gone.
        assert!(
            std::fs::read_dir(&out).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".flux")),
            "{id}"
        );
        let o = run("pw", &["check", id, "--read-data"]);
        assert!(o.status.success(), "{id}: {}", stdout(&o));

        // Names and pins live in the archive comments.
        let o = run("pw", &["pin", id, "second"]);
        assert!(
            o.status.success(),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(stdout(&run("pw", &["snapshots", id])).contains("second (pinned)"));

        let o = run("wrong", &["snapshots", id]);
        assert!(!o.status.success());
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("passphrase is wrong"),
            "{id}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    // The kit next to the local repository, and its script run without Flux.
    let folder = glob_dirs(&drive.join("flux-borg"), "neuralpunk-never-matches");
    assert!(folder.is_empty());
    let repo_dir = std::fs::read_dir(drive.join("flux-borg"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let user_dir = std::fs::read_dir(&repo_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let kit_dir = user_dir.join("local");
    let guide = std::fs::read_to_string(kit_dir.join("FLUX-RECOVERY.txt")).unwrap();
    assert!(guide.contains("export BORG_REPO="), "{guide}");
    assert!(kit_dir.join("repo/config").exists());
    assert!(kit_dir.join("flux-patterns.txt").exists());
    std::fs::write(src.join("docs/c.txt"), "gamma").unwrap();
    let o = Command::new("sh")
        .arg(kit_dir.join("flux-backup.sh"))
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("BORG_PASSPHRASE", "pw")
        .env("BORG_BASE_DIR", h.join("borg-base"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run("pw", &["--json", "snapshots", "local"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
    let o = run("pw", &["ls", "local", src.join("docs").to_str().unwrap()]);
    assert!(stdout(&o).contains("c.txt"));
    let o = run("pw", &["ls", "local", src.join("cache").to_str().unwrap()]);
    assert!(!stdout(&o).contains("junk"));

    // Delete frees space; prune keeps the pinned archive.
    let first = v[0]["id"].as_str().unwrap().to_owned();
    let o = run("pw", &["delete", "local", &first, "-y"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run("pw", &["prune", "local"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&run("pw", &["snapshots", "local"])).contains("second (pinned)"));
    if fake.is_some() {
        let server_kit = glob_files(&h.join("server"), "FLUX-RECOVERY.txt");
        assert_eq!(server_kit.len(), 1);
        let guide = std::fs::read_to_string(&server_kit[0]).unwrap();
        assert!(
            guide.contains("export BORG_REPO=ssh://localhost/"),
            "{guide}"
        );
    }
}

fn glob_files(root: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else if e.file_name() == name {
                out.push(e.path());
            }
        }
    }
    out
}

/// Snapshot directories under a Mirror tree (`…/snapshots/<id>`), sorted.
fn glob_dirs(root: &Path, parent: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().unwrap().is_dir() {
                if d.file_name().is_some_and(|n| n == parent) {
                    if !e.file_name().to_string_lossy().starts_with('.') {
                        out.push(e.path());
                    }
                } else {
                    stack.push(e.path());
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn system_scope() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let units = h.join("units");
    let calls = h.join("systemctl.log");
    let fake = h.join("fake-systemctl");
    write_executable(
        &fake,
        &format!("#!/bin/sh\necho \"$@\" >> {}\n", calls.display()),
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .arg("--system")
            .args(args)
            .env("FLUX_HOME", h)
            // Never touch the real ~/.config/borg and ~/.cache/borg.
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SYSTEMD_UNIT_DIR", &units)
            .env("FLUX_SYSTEMCTL", &fake)
            .output()
            .unwrap()
    };
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    let o = run(&[
        "profile",
        "create",
        "vault",
        "-t",
        h.join("tgt").to_str().unwrap(),
        "-s",
        "/etc",
        "--engine",
        "vault",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("key file"),
        "system Vaults are told to use a key file: {err}"
    );

    let o = run(&["config", "path", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["scope"], "system");

    // System timers run `flux --system` and talk to the system manager (no --user).
    let toml = h.join("config/profiles/vault.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(
        &toml,
        text.replace(
            "trigger = \"manual\"",
            "trigger = \"calendar\"\non_calendar = \"daily\"",
        ),
    )
    .unwrap();
    let o = run(&["schedule", "install", "vault"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let unit = std::fs::read_to_string(units.join("flux-backup-vault.service")).unwrap();
    assert!(
        unit.contains(" --system backup vault --scheduled"),
        "{unit}"
    );
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(!log.contains("--user"), "{log}");
}

/// FUSE is usable here: /dev/fuse opens and fusermount3 exists.
fn fuse_available() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fuse")
        .is_ok()
        && Command::new("fusermount3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

#[test]
fn mount_shows_snapshots_read_only() {
    if !fuse_available() {
        eprintln!("skipped: FUSE is not available");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    let big: Vec<u8> = (0..2_500_000u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::write(src.join("big.bin"), &big).unwrap();
    let engines: &[(&str, &str)] = if borg_available() {
        &[
            ("m", "mirror"),
            ("v", "vault"),
            ("r", "rsync"),
            ("b", "borg"),
        ]
    } else {
        &[("m", "mirror"), ("v", "vault"), ("r", "rsync")]
    };
    for &(id, engine) in engines {
        let tgt = h.join(format!("tgt-{id}"));
        std::fs::create_dir_all(&tgt).unwrap();
        let o = flux_pw(
            h,
            "pw",
            &[
                "profile",
                "create",
                id,
                "-t",
                tgt.to_str().unwrap(),
                "-s",
                src.to_str().unwrap(),
                "--engine",
                engine,
            ],
        );
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let o = flux_pw(h, "pw", &["backup", id]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

        let mnt = h.join(format!("mnt-{id}"));
        std::fs::create_dir_all(&mnt).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
            .args(["mount", id, mnt.to_str().unwrap()])
            .env("FLUX_HOME", h)
            // Never touch the real ~/.config/borg and ~/.cache/borg.
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_PASSWORD", "pw")
            .env("FLUX_SECRET_TOOL", "false")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let latest = mnt.join("latest");
        let t = std::time::Instant::now();
        while !latest.exists() {
            assert!(t.elapsed().as_secs() < 20, "mount didn't appear");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let inside = latest.join(src.strip_prefix("/").unwrap());
        assert_eq!(
            std::fs::read_to_string(inside.join("docs/a.txt")).unwrap(),
            "alpha"
        );
        assert_eq!(
            std::fs::read(inside.join("big.bin")).unwrap(),
            big,
            "{engine}"
        );
        assert!(
            std::fs::write(inside.join("new.txt"), "x").is_err(),
            "read-only"
        );

        let _ = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status();
        assert!(child.wait().unwrap().success());
        assert!(!latest.exists(), "unmounted");
    }
}

#[test]
fn completions_and_man_pages() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    for (shell, needle) in [
        ("bash", "_flux()"),
        ("zsh", "#compdef flux"),
        ("fish", "complete -c flux"),
    ] {
        let o = flux(h, &["completions", shell]);
        assert!(o.status.success());
        let text = stdout(&o);
        assert!(text.contains(needle), "{shell}");
        assert!(text.contains("mount"), "{shell} completes subcommands");
    }
    let dir = h.join("man");
    assert!(
        flux(h, &["manpages", dir.to_str().unwrap()])
            .status
            .success()
    );
    let page = std::fs::read_to_string(dir.join("flux-restore.1")).unwrap();
    assert!(page.contains(".TH flux-restore"), "{page}");
    assert!(dir.join("flux-schedule-install.1").is_file());
    assert!(
        !dir.join("flux-help.1").exists(),
        "no pages for clap's help"
    );
}

/// Create an executable script without this (multithreaded) test process ever holding it
/// open for writing: a child forked meanwhile by another test would share the descriptor
/// until its exec, and running the script would fail with "Text file busy".
fn write_executable(path: &std::path::Path, text: &str) {
    let src = path.with_extension("txt");
    std::fs::write(&src, text).unwrap();
    assert!(
        Command::new("install")
            .args(["-m", "0755"])
            .arg(&src)
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

/// Encryption is set up with the backup, not at its first run: `flux key init` creates the
/// encrypted Borg repository and writes the passphrase into the profile's key file, so the
/// first backup runs the way a timer runs it: no terminal, no FLUX_PASSWORD, no keyring.
#[test]
fn encryption_is_set_up_before_the_first_backup() {
    if !borg_available() {
        eprintln!("skipping: Borg 1.x is not installed");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let run = |pw: Option<&str>, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flux"));
        c.args(args)
            .env("FLUX_HOME", h)
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env_remove("FLUX_PASSWORD")
            .env("FLUX_SECRET_TOOL", "false")
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true")
            .stdin(std::process::Stdio::null());
        if let Some(pw) = pw {
            c.env("FLUX_PASSWORD", pw);
        }
        c.output().unwrap()
    };
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "alpha").unwrap();
    let drive = h.join("drive");
    std::fs::create_dir(&drive).unwrap();
    let o = run(
        None,
        &[
            "profile",
            "create",
            "p",
            "-t",
            drive.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--engine",
            "borg",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // The passphrase comes from a key file that doesn't exist yet.
    let key = h.join("keys/p.key");
    let f = h.join("config/profiles/p.toml");
    let mut text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false");
    text = text.replace(
        "[borg.password_source]\nfrom = \"keyring\"",
        &format!(
            "[borg.password_source]\nfrom = \"file\"\npath = \"{}\"",
            key.display()
        ),
    );
    std::fs::write(&f, &text).unwrap();
    assert!(text.contains("from = \"file\""), "{text}");

    // Before setup, a backup with nobody to ask can't run.
    let o = run(None, &["backup", "p"]);
    assert!(!o.status.success());

    let o = run(Some("correct horse"), &["key", "init", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        std::fs::read_to_string(&key).unwrap().trim(),
        "correct horse"
    );

    // The first backup, as a timer runs it.
    let o = run(None, &["backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(None, &["snapshots", "p", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"id\""));

    let count =
        |o: &std::process::Output| String::from_utf8_lossy(&o.stdout).matches("\"id\"").count();
    assert_eq!(count(&o), 1);
    std::fs::write(src.join("b.txt"), "beta").unwrap();
    let o = run(None, &["backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // Archives borg writes behind Flux's back show up (listings are never stale).
    let repo = walk_find(&drive, "config")
        .expect("borg repository")
        .parent()
        .unwrap()
        .to_owned();
    let o = Command::new("borg")
        .args([
            "create",
            &format!("{}::by-hand", repo.display()),
            src.to_str().unwrap(),
        ])
        .env("BORG_BASE_DIR", h.join("borg-base"))
        .env("BORG_PASSPHRASE", "correct horse")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(None, &["snapshots", "p", "--json"]);
    assert_eq!(count(&o), 3);
    // A wrong passphrase is never answered from anything cached.
    assert!(!run(Some("wrong"), &["snapshots", "p"]).status.success());
}

/// The first file named `name` under `dir`.
fn walk_find(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == name) && p.is_file() {
            return Some(p);
        }
        if p.is_dir()
            && let Some(f) = walk_find(&p, name)
        {
            return Some(f);
        }
    }
    None
}
