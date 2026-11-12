//! End-to-end tests of the `flux` binary, isolated with `FLUX_HOME`.

use std::path::Path;
use std::process::{Command, Output};

fn flux(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        // A scheduled run never waits for a click on its notification.
        .env("FLUX_NOTIFY_WAIT", "0")
        .args(args)
        .env("FLUX_HOME", home)
        // Never touch the real ~/.config/borg and ~/.cache/borg.
        .env("BORG_BASE_DIR", home.join("borg-base"))
        // Never touch the real systemd configuration from tests.
        .env("FLUX_SYSTEMD_UNIT_DIR", home.join("systemd-units"))
        .env("FLUX_SYSTEMCTL", "true")
        // …nor the real crontab or /etc/cron.d.
        .env("FLUX_CRONTAB", home.join("no-crontab"))
        // Never shut the test machine down.
        .env("FLUX_SHUTDOWN", "false")
        .env("FLUX_CRON_DIR", home.join("cron.d"))
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

    // --exact: a file added since is listed in a dry run, then removed (overwriting:
    // deleted, not kept).
    std::fs::write(src.join("docs/new.txt"), "n").unwrap();
    let docs = src.join("docs");
    let d = docs.to_str().unwrap();
    let o = flux(h, &["restore", "p", d, "--exact", "--dry-run"]);
    assert!(
        stdout(&o).contains(&format!("remove {}", docs.join("new.txt").display())),
        "{}",
        stdout(&o)
    );
    assert!(docs.join("new.txt").exists(), "a dry run changes nothing");
    let o = flux(h, &["restore", "p", d, "--exact", "--overwrite", "-y"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        stdout(&o).contains("removed 1 item that weren't in the snapshot"),
        "{}",
        stdout(&o)
    );
    assert!(!docs.join("new.txt").exists());
    assert!(docs.join("a.txt").exists());
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

/// Switching to cron and back: Flux's block in the crontab, other lines kept, and the
/// "is it due?" check that catches up a missed run.
#[test]
fn schedule_with_cron() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let units = h.join("units");
    let table = h.join("crontab.txt");
    let fake_crontab = h.join("fake-crontab");
    write_executable(
        &fake_crontab,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  -l) if [ -f {t} ]; then cat {t}; else echo 'no crontab for me' >&2; exit 1; fi ;;\n  -) cat > {t} ;;\nesac\n",
            t = table.display()
        ),
    );
    let mine = "0 7 * * * /usr/bin/backintime backup --background >/dev/null\n";
    std::fs::write(&table, mine).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SYSTEMD_UNIT_DIR", &units)
            .env("FLUX_SYSTEMCTL", "true")
            .env("FLUX_CRONTAB", &fake_crontab)
            // Never shut the test machine down.
            .env("FLUX_SHUTDOWN", "false")
            .env("FLUX_CRON_DIR", h.join("cron.d"))
            .output()
            .unwrap()
    };
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::write(h.join("src/a.txt"), "a").unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    let o = run(&[
        "profile",
        "create",
        "p",
        "-t",
        h.join("tgt").to_str().unwrap(),
        "-s",
        h.join("src").to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f).unwrap().replace(
        "trigger = \"manual\"",
        "trigger = \"calendar\"\non_calendar = \"*-*-* 07:00:00\"",
    );
    std::fs::write(&f, text).unwrap();
    assert!(run(&["schedule", "sync"]).status.success());
    assert!(units.join("flux-backup-p.timer").exists());

    // To cron: the timer goes, the crontab gets Flux's block, Back In Time's line stays.
    let o = run(&["schedule", "use", "cron"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("now run with cron instead of systemd timers"));
    assert!(!units.join("flux-backup-p.timer").exists());
    let tab = std::fs::read_to_string(&table).unwrap();
    assert!(tab.starts_with(mine), "{tab}");
    assert!(tab.contains("# profile p\n0 7 * * * "), "{tab}");
    assert!(tab.contains("*/15 * * * * "), "catch-up check: {tab}");
    assert!(
        tab.contains(" backup p --scheduled --if-due >/dev/null 2>&1"),
        "{tab}"
    );
    assert!(
        std::fs::read_to_string(h.join("config/settings.toml"))
            .unwrap()
            .contains("scheduler = \"cron\"")
    );
    let o = run(&["schedule", "status"]);
    assert!(
        stdout(&o).contains("Scheduled backups run with cron"),
        "{}",
        stdout(&o)
    );
    assert!(stdout(&o).contains("CRON"));

    // Not due yet: the schedule started just now.
    let o = run(&["backup", "p", "--scheduled", "--if-due"]);
    assert!(
        stdout(&o).contains("Skipped: not due until"),
        "{}",
        stdout(&o)
    );
    assert!(!h.join("tgt/flux-backup").exists());

    // A 07:00 missed while the computer was off runs once, late.
    let stamp = h.join("state/schedule/p.last");
    std::fs::write(&stamp, "2020-01-01T00:00:00Z").unwrap();
    let o = run(&["backup", "p", "--scheduled", "--if-due"]);
    assert!(
        o.status.success(),
        "{}{}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!stdout(&o).contains("Skipped"), "{}", stdout(&o));
    let o = run(&["backup", "p", "--scheduled", "--if-due"]);
    assert!(
        stdout(&o).contains("Skipped: not due until"),
        "once: {}",
        stdout(&o)
    );

    // With catching up off, a long-missed time is skipped instead (and only noted once).
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("catch_up = true"));
    std::fs::write(&f, text.replace("catch_up = true", "catch_up = false")).unwrap();
    assert!(run(&["schedule", "sync"]).status.success());
    assert!(!std::fs::read_to_string(&table).unwrap().contains("*/15"));
    std::fs::write(&stamp, "2020-01-01T00:00:00Z").unwrap();
    let o = run(&["backup", "p", "--scheduled", "--if-due"]);
    assert!(stdout(&o).contains("catching up is off"), "{}", stdout(&o));

    // Back to systemd: the block goes, the rest of the crontab is as it was.
    let o = run(&["schedule", "use", "systemd"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read_to_string(&table).unwrap(), mine);
    assert!(units.join("flux-backup-p.timer").exists());
    assert!(
        !std::fs::read_to_string(h.join("config/settings.toml"))
            .unwrap_or_default()
            .contains("scheduler")
    );
}

/// `--shutdown`: a minute after the backup ends, the computer powers off (here, a fake
/// `shutdown` records the call); a failure to schedule it is a warning, not a failed backup.
#[test]
fn backup_then_shut_down() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::write(h.join("src/a"), "a").unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    let calls = h.join("shutdown.log");
    let fake = h.join("fake-shutdown");
    write_executable(
        &fake,
        &format!("#!/bin/sh\necho \"$@\" >> {}\n", calls.display()),
    );
    let run = |shutdown: &std::path::Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true")
            .env("FLUX_CRONTAB", h.join("no-crontab"))
            .env("FLUX_CRON_DIR", h.join("cron.d"))
            .env("FLUX_SHUTDOWN", shutdown)
            .output()
            .unwrap()
    };
    let (s, t) = (h.join("src"), h.join("tgt"));
    assert!(
        run(
            &fake,
            &[
                "profile",
                "create",
                "p",
                "-t",
                t.to_str().unwrap(),
                "-s",
                s.to_str().unwrap()
            ]
        )
        .status
        .success()
    );

    let o = run(&fake, &["backup", "p", "--shutdown"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        std::fs::read_to_string(&calls)
            .unwrap()
            .starts_with("--poweroff +1 ")
    );

    // Without the flag: no shutdown.
    std::fs::remove_file(&calls).unwrap();
    std::fs::write(h.join("src/b"), "b").unwrap();
    assert!(run(&fake, &["backup", "p"]).status.success());
    assert!(!calls.exists());

    // Shutdown refused: the backup still counts as done, with a warning.
    std::fs::write(h.join("src/c"), "c").unwrap();
    let o = run(
        std::path::Path::new("false"),
        &["backup", "p", "--shutdown"],
    );
    assert!(
        o.status.success() || o.status.code() == Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let all = format!("{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    assert!(all.contains("couldn't shut down"), "{all}");
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
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            // Never touch the real ~/.config/borg and ~/.cache/borg.
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_SYSTEMD_UNIT_DIR", &units)
            .env("FLUX_SYSTEMCTL", &fake)
            .env("FLUX_CRONTAB", h.join("no-crontab"))
            // Never shut the test machine down.
            .env("FLUX_SHUTDOWN", "false")
            .env("FLUX_CRON_DIR", h.join("cron.d"))
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
        // A scheduled run never waits for a click on its notification.
        .env("FLUX_NOTIFY_WAIT", "0")
        .args(args)
        .env("FLUX_HOME", home)
        // Never touch the real ~/.config/borg and ~/.cache/borg.
        .env("BORG_BASE_DIR", home.join("borg-base"))
        .env("FLUX_PASSWORD", pw)
        .env("FLUX_SYSTEMD_UNIT_DIR", home.join("systemd-units"))
        .env("FLUX_SYSTEMCTL", "true")
        .env("FLUX_CRONTAB", home.join("no-crontab"))
        // Never shut the test machine down.
        .env("FLUX_SHUTDOWN", "false")
        .env("FLUX_CRON_DIR", home.join("cron.d"))
        .env("FLUX_SECRET_TOOL", "false")
        .output()
        .expect("run flux")
}

/// A Vault that keeps repair data: damage on the drive is found by `flux check
/// --read-data`, which says it can be repaired, and repaired by `flux check --repair`.
#[test]
fn check_repairs_a_damaged_vault() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    // A megabyte that doesn't compress.
    let mut x = 88172645463325252u64;
    let noise: Vec<u8> = (0..1 << 20)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(src.join("big"), &noise).unwrap();
    let pw = "s3cret pass";
    let created = flux(
        h,
        &[
            "profile",
            "create",
            "v",
            "-t",
            h.join("tgt").to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--engine",
            "vault",
        ],
    );
    assert!(created.status.success());
    let f = h.join("config/profiles/v.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false")
        .replace("[vault]", "[vault]\nparity = 10");
    assert!(text.contains("parity = 10"), "{text}");
    std::fs::write(&f, text).unwrap();
    assert!(flux_pw(h, pw, &["backup", "v"]).status.success());
    let vault = h.join("tgt/flux-vault");
    assert!(vault.join("parity/data").is_dir() && vault.join("parity/index").is_dir());

    // 20 KiB of the pack with the file's data come back as garbage.
    let pack = std::fs::read_dir(vault.join("data"))
        .unwrap()
        .flat_map(|d| std::fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().path())
        .max_by_key(|p| std::fs::metadata(p).unwrap().len())
        .unwrap();
    let good = std::fs::read(&pack).unwrap();
    let mut bad = good.clone();
    bad[300_000..320_000].fill(0x55);
    std::fs::set_permissions(&pack, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&pack, &bad).unwrap();

    let o = flux_pw(h, pw, &["check", "v", "--read-data"]);
    assert!(!o.status.success());
    let out = stdout(&o);
    assert!(out.contains("its repair data can rebuild it"), "{out}");
    assert!(out.contains("flux check v --repair"), "{out}");
    assert!(
        std::fs::read(&pack).unwrap() == bad,
        "a check alone writes nothing"
    );

    let o = flux_pw(h, pw, &["check", "v", "--repair"]);
    let out = stdout(&o);
    assert!(
        o.status.success(),
        "{out}{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        out.contains("repaired: pack ") && out.contains("0 problems, 1 repaired"),
        "{out}"
    );
    assert!(std::fs::read(&pack).unwrap() == good);
    // The config and the key got their repair data too, and all is well afterwards.
    assert!(vault.join("parity/config").is_file());
    let o = flux_pw(h, pw, &["--json", "check", "v", "--read-data"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["errors"].as_array().unwrap().len(), 0);
    let out = h.join("out");
    let o = flux_pw(
        h,
        pw,
        &[
            "restore",
            "v",
            src.to_str().unwrap(),
            "--to",
            out.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(std::fs::read(out.join("src/big")).unwrap() == noise);

    // --repair is for Vaults.
    let o = flux(h, &["check", "nope", "--repair"]);
    assert!(!o.status.success());
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

/// A Mirror on an SSH server holds what its snapshots say: a file whose name isn't UTF-8
/// comes back, a change that kept the file's size and time is stored when changes are
/// detected by checksum, and following links (which rsync wouldn't do there) is refused.
#[test]
fn ssh_mirror_stores_what_it_lists() {
    use std::os::unix::ffi::OsStrExt;
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
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("FLUX_SSH", &fake)
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true")
            .output()
            .unwrap()
    };
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let odd = src.join(std::ffi::OsStr::from_bytes(b"caf\xe9.txt"));
    let odd_name_ok = std::fs::write(&odd, "latin-1 name").is_ok(); // some filesystems refuse
    let db = src.join("db.bin");
    std::fs::write(&db, "AAAA").unwrap();
    let s = src.to_str().unwrap();
    let target = format!("localhost:{}", h.join("remote").display());
    assert!(
        run(&["profile", "create", "m", "-t", &target, "-s", s])
            .status
            .success()
    );
    let f = h.join("config/profiles/m.toml");
    let edit = |from: &str, to: &str| {
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains(from), "{from} in {text}");
        std::fs::write(&f, text.replace(from, to)).unwrap();
    };
    edit("notify = true", "notify = false");
    edit(
        "change_detection = \"metadata\"",
        "change_detection = \"checksum\"",
    );
    let o = run(&["backup", "m"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // The same size and the same time, other content.
    let mtime = std::fs::metadata(&db).unwrap().modified().unwrap();
    std::fs::write(&db, "BBBB").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&db)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let o = run(&["backup", "m"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = h.join("out");
    let o = run(&["restore", "m", s, "--to", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        std::fs::read_to_string(out.join("src/db.bin")).unwrap(),
        "BBBB",
        "the snapshot holds the new content, not a link to the old"
    );
    if odd_name_ok {
        assert_eq!(
            std::fs::read_to_string(out.join("src").join(odd.file_name().unwrap())).unwrap(),
            "latin-1 name"
        );
    }

    // Following links: refused, in words, before anything is sent.
    edit("symlinks = \"preserve\"", "symlinks = \"follow\"");
    let o = run(&["backup", "m"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("stores links as links"), "{err}");
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
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
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
    // Many small files (and an empty one): a restore from a server fetches them in
    // batches, several of them here.
    std::fs::create_dir_all(src.join("many")).unwrap();
    for i in 0..90 {
        std::fs::write(
            src.join(format!("many/n{i:02}.txt")),
            format!("small file number {i}").repeat(i + 1),
        )
        .unwrap();
    }
    std::fs::write(src.join("many/empty"), "").unwrap();
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
        for i in 0..90 {
            assert_eq!(
                std::fs::read_to_string(out.join(format!("src/many/n{i:02}.txt"))).unwrap(),
                format!("small file number {i}").repeat(i + 1),
                "{id}: n{i:02}"
            );
        }
        assert_eq!(std::fs::read(out.join("src/many/empty")).unwrap(), b"");
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

    // `check` looks at the disk, not at the listing it remembers: a snapshot (the first,
    // made by Flux) whose files are gone is a problem.
    assert!(flux(h, &["check", "plain"]).status.success());
    let snaps = glob_dirs(&target.join("flux-rsync"), "snapshots");
    let oldest = snaps.iter().min().unwrap();
    std::fs::remove_file(data(oldest, "docs/a.txt")).unwrap();
    let o = flux(h, &["check", "plain"]);
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("are missing"), "{}", stdout(&o));
    std::fs::remove_dir_all(oldest.join("data")).unwrap();
    let o = flux(h, &["check", "plain"]);
    assert!(!o.status.success(), "{}", stdout(&o));
}

/// With a size limit, rsync still counts the files it skipped; the snapshot must show
/// what it really holds, and the run must say what was left out.
#[test]
fn rsync_size_limit_counts_what_is_stored() {
    if Command::new("rsync").arg("--version").output().is_err() {
        eprintln!("skipping: no rsync");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("small.txt"), "x").unwrap();
    std::fs::write(src.join("docs/big.bin"), vec![7u8; 4096]).unwrap();
    std::fs::write(src.join("big-root.bin"), vec![7u8; 4096]).unwrap();
    let target = h.join("drive");
    std::fs::create_dir(&target).unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "p",
            "-t",
            target.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--engine",
            "rsync",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false")
        .replace("[sources]", "[sources]\nexclude_larger_than = \"1KiB\"");
    std::fs::write(&f, text).unwrap();
    // The tiny limit is legal, but validation warns about anything under 1 MiB.
    let o = flux(h, &["profile", "validate", "p"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("almost every file")
            || String::from_utf8_lossy(&o.stderr).contains("almost every file"),
        "{o:?}"
    );

    let o = flux(h, &["-v", "backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(all.contains("left out 2 files larger than"), "{all}");

    let o = flux(h, &["--json", "snapshots", "p"]);
    let snaps: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let stats = &snaps[0]["stats"];
    assert_eq!(stats["files"], 1, "{stats}");
    assert_eq!(stats["total_bytes"], 1, "{stats}");

    // Restoring brings back exactly what the snapshot holds.
    std::fs::remove_dir_all(src.join("docs")).unwrap();
    std::fs::remove_file(src.join("small.txt")).unwrap();
    let o = flux(h, &["restore", "p", src.to_str().unwrap(), "-y"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read(src.join("small.txt")).unwrap(), b"x");
    assert!(src.join("docs").is_dir());
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

/// Restoring from Borg: a file `borg extract` couldn't write out whole is an error and
/// never replaces the file that's there; and the folder above the backed-up ones, which
/// the archive knows nothing about, keeps its permissions and date.
#[test]
fn borg_restores_only_what_came_out_whole() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !borg_available() {
        eprintln!("skipping: Borg 1.x is not installed");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    // A borg whose `extract` can't write files over 1 MiB (a full disk, in effect).
    let limited = h.join("borg-limited");
    let text = h.join("borg-limited.txt");
    std::fs::write(
        &text,
        "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = extract ] && ulimit -f 2048; done\nexec borg \"$@\"\n",
    )
    .unwrap();
    assert!(
        Command::new("install")
            .args(["-m", "0755"])
            .arg(&text)
            .arg(&limited)
            .status()
            .unwrap()
            .success()
    );
    let run = |borg: Option<&Path>, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flux"));
        // A scheduled run never waits for a click on its notification.
        c.env("FLUX_NOTIFY_WAIT", "0");
        c.args(args)
            .env("FLUX_HOME", h)
            .env("BORG_BASE_DIR", h.join("borg-base"))
            .env("FLUX_PASSWORD", "pw")
            .env("FLUX_SECRET_TOOL", "false")
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("units"))
            .env("FLUX_SYSTEMCTL", "true");
        if let Some(b) = borg {
            c.env("FLUX_BORG", b);
        }
        c.output().unwrap()
    };
    let top = h.join("top");
    let src = top.join("docs");
    std::fs::create_dir_all(&src).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.join("large.bin"), &big).unwrap();
    std::fs::write(src.join("small.txt"), "small").unwrap();
    let drive = h.join("drive");
    std::fs::create_dir(&drive).unwrap();
    let o = run(
        None,
        &[
            "profile",
            "create",
            "b",
            "-t",
            drive.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--engine",
            "borg",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let f = h.join("config/profiles/b.toml");
    let text = std::fs::read_to_string(&f).unwrap();
    std::fs::write(&f, text.replace("notify = true", "notify = false")).unwrap();
    let o = run(None, &["backup", "b"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // Extraction cuts the large file short: the restore says so (exit 4, partial) and
    // the whole file that's there stays, as it is.
    std::fs::write(src.join("small.txt"), "changed").unwrap();
    let o = run(
        Some(&limited),
        &["restore", "b", src.to_str().unwrap(), "--yes"],
    );
    assert_eq!(
        o.status.code(),
        Some(4),
        "{}{}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(std::fs::read(src.join("large.bin")).unwrap(), big);
    assert_eq!(
        std::fs::read_to_string(src.join("small.txt")).unwrap(),
        "small",
        "what came out whole is restored"
    );

    // Restoring the folder above the backed-up one: it is only the way there.
    std::fs::set_permissions(&top, std::fs::Permissions::from_mode(0o1777)).unwrap();
    let before = std::fs::metadata(&top).unwrap();
    std::fs::remove_file(src.join("small.txt")).unwrap();
    let o = run(None, &["restore", "b", top.to_str().unwrap(), "--yes"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(src.join("small.txt").exists());
    let after = std::fs::metadata(&top).unwrap();
    assert_eq!(after.mode() & 0o7777, 0o1777, "its permissions are its own");
    assert_eq!(after.mtime(), before.mtime(), "and so is its date");
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
        // A scheduled run never waits for a click on its notification.
        c.env("FLUX_NOTIFY_WAIT", "0");
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
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
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
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
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
        // A scheduled run never waits for a click on its notification.
        c.env("FLUX_NOTIFY_WAIT", "0");
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

/// `flux unlock` clears locks a dead job left (no password needed), says who still holds
/// one, and `--force` clears those too; `flux vault upgrade` moves a format 1 Vault on.
#[test]
fn unlock_and_upgrade_a_vault() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::write(h.join("src/a.txt"), "alpha").unwrap();
    let tgt = h.join("tgt");
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "v",
            "-t",
            tgt.to_str().unwrap(),
            "-s",
            h.join("src").to_str().unwrap(),
            "--engine",
            "vault",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let pw = "pass phrase";
    let o = flux_pw(h, pw, &["backup", "v"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // A job on another computer died an hour ago (stale), and one is running there now.
    let locks = tgt.join("flux-vault/locks");
    let lock = |host: &str, ago_min: i64| {
        let t = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(ago_min);
        format!(r#"{{"host":"{host}","user":"bob","pid":7,"exclusive":true,"time":"{t}"}}"#)
    };
    std::fs::write(locks.join("aaaa"), lock("old-pc", 60)).unwrap();
    std::fs::write(locks.join("bbbb"), lock("nas-box", 1)).unwrap();

    let o = flux_pw(h, pw, &["backup", "v"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("bob@nas-box") && err.contains("flux unlock"),
        "{err}"
    );

    let o = flux(h, &["unlock", "v"]);
    let out = stdout(&o);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("Removed the lock of bob@old-pc"), "{out}");
    assert!(
        out.contains("In use by bob@nas-box") && out.contains("--force"),
        "{out}"
    );
    let o = flux(h, &["--json", "unlock", "v", "--force"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert!(v["still_held"].as_array().unwrap().is_empty());
    assert!(flux_pw(h, pw, &["backup", "v"]).status.success());

    // Upgrading: a new Vault is format 3 already; a format 1 config moves to 2 (only the
    // config changes: this Vault's objects are format 3's, so it's put back to open it).
    assert!(stdout(&flux(h, &["vault", "upgrade", "v"])).contains("already in format 3"));
    let config = tgt.join("flux-vault/config");
    let original = std::fs::read_to_string(&config).unwrap();
    assert!(original.contains("\"format\": 3"), "{original}");
    std::fs::write(&config, original.replace("\"format\": 3", "\"format\": 1")).unwrap();
    let out = stdout(&flux(h, &["vault", "upgrade", "v"]));
    assert!(out.contains("from format 1 to 2"), "{out}");
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("\"format\": 2")
    );
    std::fs::write(&config, &original).unwrap();
    assert!(flux_pw(h, pw, &["snapshots", "v"]).status.success());

    // Other engines have no lock files to clear.
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "m",
            "-t",
            tgt.to_str().unwrap(),
            "-s",
            h.join("src").to_str().unwrap(),
        ],
    );
    assert!(o.status.success());
    assert!(!flux(h, &["unlock", "m"]).status.success());
}

/// Vault security from the command line: stronger hashing for the password in use, and an
/// unlock file the password then needs (backups say where it should be when it's gone).
#[test]
fn key_strengthen_and_unlock_file() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::create_dir_all(h.join("stick")).unwrap();
    std::fs::write(h.join("src/a.txt"), "alpha").unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "v",
            "-t",
            h.join("tgt").to_str().unwrap(),
            "-s",
            h.join("src").to_str().unwrap(),
            "--engine",
            "vault",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let pw = "pass phrase";
    assert!(flux_pw(h, pw, &["backup", "v"]).status.success());

    let o = flux_pw(
        h,
        pw,
        &[
            "key",
            "strengthen",
            "v",
            "--memory",
            "8",
            "--passes",
            "1",
            "--threads",
            "1",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        stdout(&o).contains("custom (8 MiB, 1 passes, 1 threads)"),
        "{}",
        stdout(&o)
    );
    let o = flux_pw(h, pw, &["--json", "key", "list", "v"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v[0]["hashing"]["level"], "custom");
    assert_eq!(v[0]["hashing"]["memory_mib"], 8);
    let profile = std::fs::read_to_string(h.join("config/profiles/v.toml")).unwrap();
    assert!(profile.contains("memory_mib = 8"), "{profile}");
    // Out of range is refused before anything changes.
    let o = flux_pw(
        h,
        pw,
        &[
            "key",
            "strengthen",
            "v",
            "--memory",
            "2",
            "--passes",
            "1",
            "--threads",
            "1",
        ],
    );
    assert!(!o.status.success());

    let file = h.join("stick/flux.unlock");
    let o = flux_pw(
        h,
        pw,
        &["key", "unlock-file", "v", "create", file.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("only together with"));
    std::fs::write(h.join("src/b.txt"), "beta").unwrap();
    assert!(
        flux_pw(h, pw, &["backup", "v"]).status.success(),
        "the file is there"
    );

    let away = h.join("away");
    std::fs::rename(&file, &away).unwrap();
    std::fs::write(h.join("src/c.txt"), "gamma").unwrap();
    let o = flux_pw(h, pw, &["backup", "v"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("unlock file") && err.contains("flux.unlock"),
        "{err}"
    );

    std::fs::rename(&away, &file).unwrap();
    let o = flux_pw(h, pw, &["key", "unlock-file", "v", "remove"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::fs::remove_file(&file).unwrap();
    assert!(
        flux_pw(h, pw, &["backup", "v"]).status.success(),
        "the password alone again"
    );
}

/// Source drives, simulated with `FLUX_FAKE_MOUNTS` (a file of "UUID FOLDER" lines the
/// test edits to plug and unplug the photos drive).
#[test]
fn source_drives_are_recognised_when_missing() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let (src, photos, mounts) = (h.join("src"), h.join("photos"), h.join("mounts"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::write(src.join("notes.txt"), "v1").unwrap();
    std::fs::write(photos.join("a.jpg"), "picture").unwrap();
    let plug = |on: bool| {
        let line = if on {
            format!("PHOTO-UUID {}\n", photos.display())
        } else {
            String::new()
        };
        std::fs::write(&mounts, line).unwrap();
    };
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("FLUX_FAKE_MOUNTS", &mounts)
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("systemd-units"))
            .env("FLUX_SYSTEMCTL", "true")
            .env("FLUX_CRONTAB", h.join("no-crontab"))
            .env("FLUX_CRON_DIR", h.join("cron.d"))
            .env("FLUX_SHUTDOWN", "false")
            .env_remove("FLUX_LOG")
            .output()
            .unwrap()
    };
    let profile_file = h.join("config/profiles/p.toml");
    let set = |from: &str, to: &str| {
        let t = std::fs::read_to_string(&profile_file).unwrap();
        assert!(t.contains(from), "{t}");
        std::fs::write(&profile_file, t.replace(from, to)).unwrap();
    };

    plug(true);
    let o = run(&[
        "profile",
        "create",
        "p",
        "-t",
        h.join("tgt").to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
        "-s",
        photos.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(&profile_file).unwrap();
    assert!(
        text.contains("fs_uuid = \"PHOTO-UUID\""),
        "the photos drive is recorded:\n{text}"
    );
    set("notify = true", "notify = false");
    assert!(run(&["backup", "p"]).status.success());

    // Unplugged (skip, the default): the backup keeps the photos and says so.
    plug(false);
    std::fs::write(src.join("notes.txt"), "v2").unwrap();
    let o = run(&["--json", "backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let snap = &v["result"]["snapshot"];
    assert_eq!(snap["stats"]["deleted"], 0, "{v}");
    assert_eq!(snap["sources_missing"][0]["path"], photos.to_str().unwrap());
    let id = snap["id"].as_str().unwrap();
    let ls = stdout(&run(&["ls", "p", "-s", id, photos.to_str().unwrap()]));
    assert!(ls.contains("a.jpg"), "{ls}");
    let log = run(&["--json", "log", "p"]);
    let runs: serde_json::Value = serde_json::from_slice(&log.stdout).unwrap();
    assert_eq!(
        runs[0]["sources_missing"][0],
        photos.to_str().unwrap(),
        "{runs}"
    );

    // Fail: refused, in words.
    set("when_missing = \"skip\"", "when_missing = \"fail\"");
    let o = run(&["backup", "p"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("isn't there") && err.contains("PHOTO-UUID"),
        "{err}"
    );

    // Wait: in vain (fails after the limit), then plugged in while waiting (runs).
    set(
        "when_missing = \"fail\"",
        "when_missing = \"wait\"\nwait_for = \"1s\"",
    );
    let o = run(&["backup", "p"]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("waited 1s"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    set("wait_for = \"1s\"", "wait_for = \"20s\"");
    std::fs::write(src.join("notes.txt"), "v3").unwrap();
    let child = std::thread::spawn({
        let plug_later = (mounts.clone(), photos.clone());
        move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            std::fs::write(
                &plug_later.0,
                format!("PHOTO-UUID {}\n", plug_later.1.display()),
            )
            .unwrap();
        }
    });
    let o = run(&["--json", "backup", "p"]);
    child.join().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(
        v["result"]["snapshot"].get("sources_missing").is_none(),
        "{v}"
    );

    // Only the photos, unplugged: nothing to back up, so the run is skipped.
    plug(true);
    let o = run(&[
        "profile",
        "create",
        "only",
        "-t",
        h.join("tgt").to_str().unwrap(),
        "-s",
        photos.to_str().unwrap(),
    ]);
    assert!(o.status.success());
    plug(false);
    let o = run(&["backup", "only"]);
    let all = format!("{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    assert!(all.contains("nothing to back up"), "{all}");

    // rsync can't keep files from before: the source is left out, and said so.
    plug(true);
    let o = run(&[
        "profile",
        "create",
        "r",
        "-t",
        h.join("tgt-r").to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
        "-s",
        photos.to_str().unwrap(),
        "--engine",
        "rsync",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::fs::create_dir_all(h.join("tgt-r")).unwrap();
    plug(false);
    let o = run(&["backup", "r"]);
    let all = format!("{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success(), "{all}");
    assert!(all.contains("left out of this snapshot"), "{all}");
}

/// Notes and tags, on every engine: kept with the snapshot, shown, found by tag, and
/// never a reason to keep a snapshot (a tagged one is pruned like any other).
#[test]
fn notes_and_tags_on_every_engine() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
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
        // Retention: only the newest snapshot (plus anything named or pinned).
        let f = h.join(format!("config/profiles/{id}.toml"));
        let t = std::fs::read_to_string(&f).unwrap();
        let t = t
            .replace("notify = true", "notify = false")
            .replace("keep_within = \"1day\"\n", "")
            .replace("keep_daily = 7", "keep_daily = 0")
            .replace("keep_weekly = 4", "keep_weekly = 0")
            .replace("keep_monthly = 12", "keep_monthly = 0")
            .replace("keep_yearly = 5", "keep_yearly = 0")
            // Exact pruning: without unusual-change detection (which keeps one more).
            .replacen("\n[", "\nunusual_changes = \"off\"\n\n[", 1);
        std::fs::write(&f, t).unwrap();

        std::fs::write(src.join("a.txt"), format!("{id} one")).unwrap();
        assert!(
            flux_pw(h, "pw", &["backup", id]).status.success(),
            "{engine}"
        );
        let o = flux_pw(h, "pw", &["note", id, "latest", "before", "the", "upgrade"]);
        assert!(
            o.status.success(),
            "{engine}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = flux_pw(h, "pw", &["tag", id, "latest", "Taxes", "release", "taxes"]);
        assert!(
            stdout(&o).contains("tagged Taxes, release"),
            "{engine}: {}",
            stdout(&o)
        );
        let o = flux_pw(h, "pw", &["tag", id, "latest", "release", "--remove"]);
        assert!(
            stdout(&o).contains("tagged Taxes."),
            "{engine}: {}",
            stdout(&o)
        );

        let info = stdout(&flux_pw(h, "pw", &["info", id, "latest"]));
        assert!(
            info.contains("before the upgrade") && info.contains("Taxes"),
            "{engine}: {info}"
        );
        let o = flux_pw(h, "pw", &["--json", "snapshots", id, "--tag", "taxes"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1, "{engine}: {v}");
        assert_eq!(v[0]["note"], "before the upgrade");
        let list = stdout(&flux_pw(h, "pw", &["snapshots", id]));
        assert!(
            list.contains("#Taxes · before the upgrade"),
            "{engine}: {list}"
        );

        // A newer snapshot: retention removes the tagged one (tags don't keep it).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(src.join("a.txt"), format!("{id} two")).unwrap();
        let o = flux_pw(h, "pw", &["backup", id]);
        assert!(
            o.status.success(),
            "{engine}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = flux_pw(h, "pw", &["--json", "snapshots", id]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(
            v.as_array().unwrap().len(),
            1,
            "{engine}: tagged snapshot pruned: {v}"
        );
        assert!(v[0].get("note").is_none());
        let o = flux_pw(h, "pw", &["note", id, "latest", "--clear"]);
        assert!(stdout(&o).contains("Removed the note"), "{engine}");
    }
}

/// One profile, three destinations (Mirror here, a Vault and rsync elsewhere): every
/// backup goes to all of them, each has its own snapshots (`home+usb`), and one failing
/// doesn't stop the others.
#[test]
fn several_destinations() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "one").unwrap();
    for t in ["tgt-main", "tgt-usb", "tgt-plain"] {
        std::fs::create_dir_all(h.join(t)).unwrap();
    }
    let ok = |o: &Output| {
        assert!(
            o.status.success(),
            "{}{}",
            stdout(o),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    ok(&flux_pw(
        h,
        "pw",
        &[
            "profile",
            "create",
            "home",
            "-t",
            h.join("tgt-main").to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
        ],
    ));
    let f = h.join("config/profiles/home.toml");
    std::fs::write(
        &f,
        std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false"),
    )
    .unwrap();
    ok(&flux_pw(
        h,
        "pw",
        &[
            "profile",
            "destination",
            "add",
            "home",
            "usb",
            "-t",
            h.join("tgt-usb").to_str().unwrap(),
            "--engine",
            "vault",
            "--label",
            "USB stick",
        ],
    ));
    ok(&flux_pw(
        h,
        "pw",
        &[
            "profile",
            "destination",
            "add",
            "home",
            "plain",
            "-t",
            h.join("tgt-plain").to_str().unwrap(),
            "--engine",
            "rsync",
            "--offsite",
        ],
    ));
    // Same place twice, or a name taken: refused.
    assert!(
        !flux_pw(
            h,
            "pw",
            &[
                "profile",
                "destination",
                "add",
                "home",
                "again",
                "-t",
                h.join("tgt-usb").to_str().unwrap(),
                "--engine",
                "vault"
            ]
        )
        .status
        .success()
    );
    assert!(
        !flux_pw(
            h,
            "pw",
            &[
                "profile",
                "destination",
                "add",
                "home",
                "usb",
                "-t",
                h.join("x").to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let list = stdout(&flux_pw(
        h,
        "pw",
        &["profile", "destination", "list", "home"],
    ));
    assert!(
        list.contains("home+usb") && list.contains("USB stick") && list.contains("offsite"),
        "{list}"
    );
    // A destination isn't a profile of its own.
    assert!(
        !flux_pw(
            h,
            "pw",
            &["profile", "create", "home+x", "-t", "/tmp", "-s", "/tmp"]
        )
        .status
        .success()
    );

    let o = flux_pw(h, "pw", &["backup", "home"]);
    ok(&o);
    let out = stdout(&o);
    assert!(
        out.contains("✓ usb: snapshot") && out.contains("✓ plain: snapshot"),
        "{out}"
    );
    for id in ["home", "home+usb", "home+plain"] {
        let o = flux_pw(h, "pw", &["--json", "snapshots", id]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|_| panic!("{id}: {}", String::from_utf8_lossy(&o.stderr)));
        assert_eq!(v.as_array().unwrap().len(), 1, "{id}: {v}");
    }
    let o = flux_pw(h, "pw", &["--json", "log", "home"]);
    let runs: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let d = runs[0]["destinations"].as_array().unwrap();
    assert_eq!(d.len(), 2);
    assert_eq!(d[0]["status"], "completed");

    // The USB stick gone: the main backup and the rsync copy still happen.
    std::fs::remove_dir_all(h.join("tgt-usb")).unwrap();
    std::fs::write(src.join("a.txt"), "second version").unwrap(); // another size: rsync compares size and time
    let o = flux_pw(h, "pw", &["backup", "home"]);
    assert_eq!(
        o.status.code(),
        Some(4),
        "a failed destination is a partial run"
    );
    let out = stdout(&o);
    assert!(
        out.contains("✗ usb: failed") && out.contains("✓ plain: snapshot"),
        "{out}"
    );
    let o = flux_pw(h, "pw", &["--json", "snapshots", "home+plain"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);

    // Restoring from a destination works like from any backup.
    let o = flux_pw(
        h,
        "pw",
        &[
            "restore",
            "home+plain",
            "-s",
            "latest",
            src.join("a.txt").to_str().unwrap(),
            "--to",
            h.join("back").to_str().unwrap(),
            "-y",
        ],
    );
    ok(&o);
    // Backing up one destination alone; removing one keeps its backups.
    std::fs::create_dir_all(h.join("tgt-usb")).unwrap();
    ok(&flux_pw(h, "pw", &["backup", "home+usb"]));
    ok(&flux_pw(
        h,
        "pw",
        &["profile", "destination", "remove", "home", "usb"],
    ));
    assert!(h.join("tgt-usb/flux-vault/config").exists());
    assert!(
        !flux_pw(h, "pw", &["snapshots", "home+usb"])
            .status
            .success()
    );
}

/// Copies of the main backup (`--replicate`): the sources are read once, the copy holds the
/// same snapshots (a Vault copy opens with the main Vault's password, here from a fake
/// keyring that only knows the main profile), and what can't be copied is refused.
#[test]
fn replicated_destinations() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "one").unwrap();
    std::fs::write(src.join("big.bin"), vec![7u8; 300_000]).unwrap();
    for t in ["v-main", "v-usb", "m-main", "m-copy"] {
        std::fs::create_dir_all(h.join(t)).unwrap();
    }
    // A keyring that has the password of "vault" only.
    let fake = h.join("secret-tool");
    assert!(flux_transport_executable(
        &fake,
        "#!/bin/sh\nfor a; do last=$a; done\n[ \"$last\" = vault ] && printf pw && exit 0\nexit 1\n"
    ));
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("FLUX_SECRET_TOOL", &fake)
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("systemd-units"))
            .env("FLUX_SYSTEMCTL", "true")
            .env("FLUX_CRONTAB", h.join("no-crontab"))
            .env("FLUX_CRON_DIR", h.join("cron.d"))
            .env("FLUX_SHUTDOWN", "false")
            .env_remove("FLUX_PASSWORD")
            .env_remove("FLUX_LOG")
            .output()
            .unwrap()
    };
    let ok = |o: &Output| {
        assert!(
            o.status.success(),
            "{}{}",
            stdout(o),
            String::from_utf8_lossy(&o.stderr)
        )
    };

    // A Vault, copied to a "USB stick".
    ok(&run(&[
        "profile",
        "create",
        "vault",
        "-t",
        h.join("v-main").to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
        "--engine",
        "vault",
    ]));
    let f = h.join("config/profiles/vault.toml");
    std::fs::write(
        &f,
        std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false"),
    )
    .unwrap();
    ok(&run(&[
        "profile",
        "destination",
        "add",
        "vault",
        "usb",
        "-t",
        h.join("v-usb").to_str().unwrap(),
        "--replicate",
    ]));
    assert!(stdout(&run(&["profile", "destination", "list", "vault"])).contains("copy of main"));
    // The first backup creates the Vault: its password is typed (here, given).
    let o = flux_pw(h, "pw", &["backup", "vault"]);
    ok(&o);
    assert!(stdout(&o).contains("✓ usb: snapshot"), "{}", stdout(&o));
    // From then on the keyring has it (and only under "vault").
    std::fs::write(src.join("new.txt"), "new").unwrap();
    let o = run(&["backup", "vault"]);
    ok(&o);
    let ids = |id: &str| -> Vec<String> {
        let o = run(&["--json", "snapshots", id]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|_| panic!("{id}: {}", String::from_utf8_lossy(&o.stderr)));
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        ids("vault"),
        ids("vault+usb"),
        "the same snapshots, opened with the main password"
    );
    ok(&run(&[
        "restore",
        "vault+usb",
        src.join("a.txt").to_str().unwrap(),
        "--to",
        h.join("back").to_str().unwrap(),
        "-y",
    ]));

    // Mirror, copied to another folder: unchanged files hard-linked in the copy too.
    ok(&run(&[
        "profile",
        "create",
        "plain",
        "-t",
        h.join("m-main").to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
    ]));
    let f = h.join("config/profiles/plain.toml");
    std::fs::write(
        &f,
        std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false"),
    )
    .unwrap();
    ok(&run(&[
        "profile",
        "destination",
        "add",
        "plain",
        "copy",
        "-t",
        h.join("m-copy").to_str().unwrap(),
        "--replicate",
    ]));
    ok(&run(&["backup", "plain"]));
    std::fs::write(src.join("a.txt"), "second version").unwrap();
    ok(&run(&["backup", "plain"]));
    let copy_ids = ids("plain+copy");
    assert_eq!(ids("plain"), copy_ids);
    let find = |name: &str| -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![h.join("m-copy")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() && !p.is_symlink() {
                    stack.push(p)
                } else if p.file_name().is_some_and(|n| n == name) {
                    out.push(p)
                }
            }
        }
        out
    };
    use std::os::unix::fs::MetadataExt;
    let bigs = find("big.bin");
    assert_eq!(bigs.len(), 2, "{bigs:?}");
    assert_eq!(
        std::fs::metadata(&bigs[0]).unwrap().ino(),
        std::fs::metadata(&bigs[1]).unwrap().ino(),
        "the unchanged file is one file in the copy too"
    );

    // Not copyable: another engine, Borg, or plain folders to a server.
    for (t, extra) in [("x1", &["--engine", "vault"][..]), ("x2", &[][..])] {
        let tgt = if t == "x2" {
            "me@nas:/srv/x2".to_string()
        } else {
            h.join(t).display().to_string()
        };
        let mut args = vec![
            "profile",
            "destination",
            "add",
            "plain",
            t,
            "-t",
            &tgt,
            "--replicate",
        ];
        args.extend_from_slice(extra);
        let o = run(&args);
        assert!(!o.status.success(), "{t}: {}", stdout(&o));
    }
}

/// Create an executable script (written by `install`, so this process never holds it open
/// for writing: "Text file busy" otherwise).
fn flux_transport_executable(path: &Path, text: &str) -> bool {
    let src = path.with_extension("txt");
    std::fs::write(&src, text).unwrap();
    Command::new("install")
        .args(["-m", "0755"])
        .arg(&src)
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

/// Wi-Fi rules for scheduled backups to a server, with a fake `nmcli` saying which
/// network this is (and `ssh` replaced by `false`: nothing really connects).
#[test]
fn wifi_rules_for_scheduled_backups() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir_all(h.join("src")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    std::fs::write(h.join("src/a"), "a").unwrap();
    let nmcli = h.join("nmcli");
    let on = |ssid: &str| {
        let _ = std::fs::remove_file(&nmcli);
        assert!(flux_transport_executable(
            &nmcli,
            &format!("#!/bin/sh\necho 'no:Neighbour'\necho 'yes:{ssid}'\n")
        ));
    };
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            // A scheduled run never waits for a click on its notification.
            .env("FLUX_NOTIFY_WAIT", "0")
            .args(args)
            .env("FLUX_HOME", h)
            .env("FLUX_NMCLI", &nmcli)
            .env("FLUX_SSH", "/bin/false")
            .env("FLUX_SYSTEMD_UNIT_DIR", h.join("systemd-units"))
            .env("FLUX_SYSTEMCTL", "true")
            .env("FLUX_CRONTAB", h.join("no-crontab"))
            .env("FLUX_CRON_DIR", h.join("cron.d"))
            .env("FLUX_SHUTDOWN", "false")
            .env("FLUX_PASSWORD", "pw")
            .env_remove("FLUX_LOG")
            .output()
            .unwrap()
    };
    let all = |o: &Output| format!("{}{}", stdout(o), String::from_utf8_lossy(&o.stderr));
    assert!(
        run(&[
            "profile",
            "create",
            "srv",
            "-t",
            "me@nas:/srv/flux",
            "-s",
            h.join("src").to_str().unwrap()
        ])
        .status
        .success()
    );
    let f = h.join("config/profiles/srv.toml");
    let add = "\n[network]\nwifi_only = [\"Home\"]\nwifi_never = [\"Phone\"]\n";
    std::fs::write(&f, std::fs::read_to_string(&f).unwrap() + add).unwrap();

    on("Café");
    let o = run(&["backup", "srv", "--scheduled"]);
    assert!(
        all(&o).contains("Skipped: on the Wi-Fi network “Café”, not one of “Home”"),
        "{}",
        all(&o)
    );
    on("Phone");
    assert!(all(&run(&["backup", "srv", "--scheduled"])).contains("where backups don't run"));
    // On an allowed network, or started by hand, it goes ahead (and fails to connect,
    // since there's no server here).
    on("Home");
    let o = run(&["backup", "srv", "--scheduled"]);
    assert!(!all(&o).contains("Wi-Fi"), "{}", all(&o));
    on("Café");
    assert!(
        !all(&run(&["backup", "srv"])).contains("Wi-Fi"),
        "by hand: no rule"
    );

    // A server among several places: only that place is skipped.
    assert!(
        run(&[
            "profile",
            "create",
            "home",
            "-t",
            h.join("tgt").to_str().unwrap(),
            "-s",
            h.join("src").to_str().unwrap()
        ])
        .status
        .success()
    );
    let f = h.join("config/profiles/home.toml");
    std::fs::write(
        &f,
        std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false")
            + add,
    )
    .unwrap();
    assert!(
        run(&[
            "profile",
            "destination",
            "add",
            "home",
            "nas",
            "-t",
            "me@nas:/srv/flux"
        ])
        .status
        .success()
    );
    let o = run(&["backup", "home", "--scheduled"]);
    let out = all(&o);
    assert!(
        out.contains("✓ Snapshot") && out.contains("nas: skipped"),
        "{out}"
    );
}

/// `flux restore-test` by hand: a sample, everything, its history.
#[test]
fn restore_test_by_hand() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::create_dir_all(h.join("src/sub")).unwrap();
    std::fs::create_dir_all(h.join("tgt")).unwrap();
    for i in 0..6 {
        std::fs::write(h.join(format!("src/f{i}")), format!("file {i}")).unwrap();
    }
    std::fs::write(h.join("src/sub/f0"), "same name, another folder").unwrap();
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
                h.join("src").to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let f = h.join("config/profiles/p.toml");
    std::fs::write(
        &f,
        std::fs::read_to_string(&f)
            .unwrap()
            .replace("notify = true", "notify = false"),
    )
    .unwrap();
    assert!(flux(h, &["backup", "p"]).status.success());

    let o = flux(h, &["restore-test", "p", "--files", "3"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        stdout(&o).contains("✓ restore test passed: 3 files"),
        "{}",
        stdout(&o)
    );
    // Two files named f0 in different folders: both restored and checked.
    let o = flux(h, &["--json", "restore-test", "p", "--full"]);
    assert!(o.status.success());
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        (r["result"].as_str(), r["tested"].as_u64()),
        (Some("passed"), Some(7))
    );
    // Sized by what it must prove: damage to 30% of the 7 files (3 of them) or more.
    let o = flux(h, &["--json", "restore-test", "p", "--damage", "30%"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(r["mode"].as_str(), Some("confidence"));
    assert_eq!(
        r["tested"].as_u64(),
        Some(5),
        "C(4,5)/C(7,5) = 0 ≤ 1%; four would miss 1 time in 35"
    );
    assert_eq!(
        (
            r["guarantee"]["damaged"].as_u64(),
            r["guarantee"]["certainty"].as_str()
        ),
        (Some(3), Some("99%"))
    );
    let o = flux(
        h,
        &["restore-test", "p", "--damage", "30%", "--certainty", "90%"],
    );
    assert!(
        stdout(&o).contains("If 30% or more of its 7 files were damaged, this test would have found it (90% certain)."),
        "{}",
        stdout(&o)
    );
    let o = flux(h, &["restore-test", "p", "--damage", "80%"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("0.01% to 50%"));
    let o = flux(h, &["--json", "restore-test", "p", "--history"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 4);
    assert!(stdout(&flux(h, &["restore-test", "p", "--history"])).contains("passed"));
}

#[test]
fn recover_backups_on_a_new_computer() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = (t.path().join("old"), t.path().join("new"));
    let (src, tgt) = (t.path().join("src"), t.path().join("tgt"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&tgt).unwrap();
    std::fs::write(src.join("a.txt"), "alpha").unwrap();
    let pw = "s3cret pass";
    let create = [
        "profile",
        "create",
        "home",
        "-t",
        tgt.to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
        "--engine",
        "vault",
    ];
    assert!(flux(&old, &create).status.success());
    let f = old.join("config/profiles/home.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false");
    std::fs::write(&f, text).unwrap();
    // The installed recovery tool (here a stand-in) is kept next to the Vault.
    let fake_tool = t.path().join("tool-build");
    std::fs::write(&fake_tool, "#!/bin/sh\n").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_flux"))
        // A scheduled run never waits for a click on its notification.
        .env("FLUX_NOTIFY_WAIT", "0")
        .args(["backup", "home"])
        .env("FLUX_HOME", &old)
        .env("FLUX_PASSWORD", pw)
        .env("FLUX_SECRET_TOOL", "false")
        .env("FLUX_VAULT_RECOVER", &fake_tool)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(tgt.join("flux-vault-recover").is_file());
    assert!(tgt.join("FLUX-VAULT-RECOVERY.txt").is_file());

    // The new computer: without the password, the Vault's backups can't be listed.
    let o = flux(&new, &["recover", tgt.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("encrypted Vault"));
    let o = flux_pw(&new, pw, &["recover", tgt.to_str().unwrap()]);
    let out = stdout(&o);
    assert!(
        out.contains("1. home (Vault)") && out.contains("1 snapshot"),
        "{out}"
    );
    assert!(out.contains("settings kept with it"), "{out}");
    // The flux-vault folder itself can be given too.
    let o = flux_pw(
        &new,
        pw,
        &[
            "--json",
            "recover",
            tgt.join("flux-vault").to_str().unwrap(),
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["found"][0]["kept"]["profile"], "home");

    let o = flux_pw(
        &new,
        pw,
        &[
            "recover",
            tgt.to_str().unwrap(),
            "--pick",
            "1",
            "--id",
            "laptop",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("Recovered “"));
    let saved = std::fs::read_to_string(new.join("config/profiles/laptop.toml")).unwrap();
    assert!(
        saved.contains("[origin]") && saved.contains("profile = \"home\""),
        "{saved}"
    );
    let o = flux_pw(&new, pw, &["snapshots", "laptop"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        stdout(&o).lines().filter(|l| l.contains(" ok ")).count(),
        1,
        "{}",
        stdout(&o)
    );
    // Restoring works from the recovered profile.
    let to = t.path().join("restored");
    let o = flux_pw(
        &new,
        pw,
        &[
            "restore",
            "laptop",
            "--to",
            to.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        glob_files(&to, "a.txt")
            .iter()
            .any(|p| std::fs::read_to_string(p).unwrap() == "alpha")
    );
    assert!(
        !flux(&new, &["recover", tgt.to_str().unwrap(), "--pick", "9"])
            .status
            .success()
    );
}

#[test]
fn export_snapshots_as_archives() {
    let t = tempfile::tempdir().unwrap();
    let h = t.path();
    let (src, tgt) = (h.join("src"), h.join("tgt"));
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::create_dir_all(&tgt).unwrap();
    std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    std::fs::write(src.join("b.txt"), "beta").unwrap();
    std::os::unix::fs::symlink("a.txt", src.join("docs/link")).unwrap();
    let create = [
        "profile",
        "create",
        "p",
        "-t",
        tgt.to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
    ];
    assert!(flux(h, &create).status.success());
    assert!(flux(h, &["backup", "p"]).status.success());

    let tar = h.join("all.tar.zst");
    let o = flux(
        h,
        &[
            "export",
            "p",
            tar.to_str().unwrap(),
            "--checksums",
            "sha256",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = stdout(&o);
    assert!(
        out.contains("Exported 2 files") && out.contains("Checksums:"),
        "{out}"
    );
    assert!(tar.is_file() && h.join("all.tar.zst.sha256").is_file());

    let zip = h.join("docs.zip");
    let docs = src.join("docs");
    let o = flux(
        h,
        &["export", "p", zip.to_str().unwrap(), docs.to_str().unwrap()],
    );
    let out = stdout(&o);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        out.contains("Exported 1 file ") && out.contains("1 symbolic link left out"),
        "{out}"
    );
    assert!(out.contains("ZIP keeps files"), "{out}");

    // An existing archive, an unknown kind, a path that isn't backed up: refused.
    assert!(
        !flux(h, &["export", "p", zip.to_str().unwrap()])
            .status
            .success()
    );
    let o = flux(h, &["export", "p", h.join("x.rar").to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&o.stderr).contains(".tar.zst"));
    let o = flux(
        h,
        &["export", "p", h.join("y.zip").to_str().unwrap(), "/nowhere"],
    );
    assert!(!o.status.success());
}

#[test]
fn explain_what_is_backed_up() {
    let t = tempfile::tempdir().unwrap();
    let h = t.path();
    let src = h.join("src");
    std::fs::create_dir_all(src.join("app/node_modules/x")).unwrap();
    let tgt = h.join("tgt");
    std::fs::create_dir_all(&tgt).unwrap();
    std::fs::write(src.join("app/main.rs"), "fn main() {}").unwrap();
    std::fs::write(src.join("app/node_modules/x/lib.js"), "x".repeat(100)).unwrap();
    std::fs::write(src.join("notes.tmp"), "t").unwrap();
    let create = [
        "profile",
        "create",
        "p",
        "-t",
        tgt.to_str().unwrap(),
        "-s",
        src.to_str().unwrap(),
    ];
    assert!(flux(h, &create).status.success());
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f).unwrap();
    let text = if text.contains("exclude = [") {
        text.replacen(
            "exclude = [",
            "exclude = [\"node_modules/\", \"*.tmp\", ",
            1,
        )
    } else {
        text.replacen(
            "[sources]\n",
            "[sources]\nexclude = [\"node_modules/\", \"*.tmp\"]\n",
            1,
        )
    };
    std::fs::write(&f, text).unwrap();

    let o = flux(
        h,
        &[
            "explain",
            "p",
            src.join("app/main.rs").to_str().unwrap(),
            src.join("notes.tmp").to_str().unwrap(),
            "/etc/hostname",
        ],
    );
    let out = stdout(&o);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("Backed up: it's in"), "{out}");
    assert!(out.contains("Left out by the rule *.tmp"), "{out}");
    assert!(out.contains("isn't in any of the folders"), "{out}");

    let o = flux(
        h,
        &["explain", "p", src.join("app").to_str().unwrap(), "--list"],
    );
    let out = stdout(&o);
    assert!(
        out.contains("✓ main.rs") && out.contains("✗ node_modules/"),
        "{out}"
    );
    assert!(out.contains("Left out by the rule node_modules/"), "{out}");

    let o = flux(
        h,
        &[
            "--json",
            "explain",
            "p",
            src.join("app/node_modules/x/lib.js").to_str().unwrap(),
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v[0]["backed_up"], false);
    assert_eq!(v[0]["why"]["kind"], "excluded");
    assert_eq!(v[0]["why"]["rule"], "node_modules/");
}

#[test]
fn profiles_from_templates() {
    let t = tempfile::tempdir().unwrap();
    let h = t.path();
    let tgt = h.join("tgt");
    std::fs::create_dir_all(&tgt).unwrap();
    let out = stdout(&flux(h, &["profile", "templates"]));
    assert!(
        out.contains("development") && out.contains("Photos"),
        "{out}"
    );
    let src = h.join("code");
    std::fs::create_dir_all(&src).unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "dev",
            "-t",
            tgt.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--template",
            "development",
            "-x",
            "*.log",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(h.join("config/profiles/dev.toml")).unwrap();
    assert!(
        text.contains("node_modules/")
            && text.contains("*.log")
            && text.contains("trigger = \"interval\""),
        "{text}"
    );
    assert!(text.contains("name = \"Projects\""), "{text}");
    // A backup created with a schedule gets its timer there and then: without one it
    // would never run.
    assert!(
        stdout(&o).contains("Updated the schedule"),
        "{}",
        stdout(&o)
    );
    assert!(
        h.join("systemd-units/flux-backup-dev.timer").exists()
            || std::fs::read_dir(h.join("systemd-units"))
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains("dev")),
        "a timer unit for the new profile"
    );
    // Saved as yours, then used by name.
    let o = flux(h, &["profile", "save-template", "dev", "Work laptop"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&flux(h, &["profile", "templates"])).contains("Work laptop (yours)"));
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "dev2",
            "-t",
            tgt.to_str().unwrap(),
            "--template",
            "Work laptop",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        std::fs::read_to_string(h.join("config/profiles/dev2.toml"))
            .unwrap()
            .contains("*.log")
    );
    assert!(
        flux(h, &["profile", "delete-template", "Work laptop"])
            .status
            .success()
    );
    assert!(
        !flux(
            h,
            &[
                "profile",
                "create",
                "x",
                "-t",
                tgt.to_str().unwrap(),
                "--template",
                "nope"
            ]
        )
        .status
        .success()
    );
    assert!(
        !flux(h, &["profile", "create", "y", "-t", tgt.to_str().unwrap()])
            .status
            .success(),
        "sources or a template"
    );
}

/// Copy a folder, keeping symbolic links as links.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let t = e.file_type().unwrap();
        if t.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src).unwrap(), &dst).unwrap();
        } else if t.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn sha256(path: &Path) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(std::fs::read(path).unwrap());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Profiles and backups written by every release of Flux (made by that release itself,
/// see crates/flux-runner/tests/fixtures/README.md) are read by this one: the profile
/// files validate, the snapshots are listed and restored as they were, and a new backup
/// carries on into the old repository.
#[test]
fn backups_from_every_release_are_read() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../flux-runner/tests/fixtures");
    let mut versions: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("expected.json").is_file())
        .map(|e| e.path())
        .collect();
    versions.sort();
    assert!(versions.len() >= 4, "{versions:?}");
    for dir in versions {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let exp: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        let t = tempfile::tempdir().unwrap();
        let (home, tgt, src) = (
            t.path().join("home"),
            t.path().join("tgt"),
            t.path().join("src"),
        );
        copy_tree(&dir.join("tgt"), &tgt);
        std::fs::create_dir_all(src.join("new")).unwrap();
        std::fs::write(src.join("new/file.txt"), "made by this Flux").unwrap();
        let profiles = home.join("config/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        for engine in exp["engines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
        {
            if engine == "borg" && !borg_available() {
                continue;
            }
            let what = format!("{name} {engine}");
            // The profile as that release wrote it, moved here and recovered as its origin.
            let text = std::fs::read_to_string(dir.join(format!("profiles/{engine}.toml")))
                .unwrap()
                .replace("/srv/tgt", tgt.to_str().unwrap())
                .replace("/srv/src", src.to_str().unwrap())
                .replace("notify = true", "notify = false");
            let text = format!(
                "{text}\n[origin]\nhost = \"fixture-host\"\nuser = \"root\"\nprofile = \"{engine}\"\n"
            );
            std::fs::write(profiles.join(format!("{engine}.toml")), text).unwrap();
            let run = |args: &[&str]| {
                Command::new(env!("CARGO_BIN_EXE_flux"))
                    // A scheduled run never waits for a click on its notification.
                    .env("FLUX_NOTIFY_WAIT", "0")
                    .args(args)
                    .env("FLUX_HOME", &home)
                    .env("FLUX_PASSWORD", "fixture")
                    .env("BORG_BASE_DIR", home.join("borg-base"))
                    .env("FLUX_SECRET_TOOL", "false")
                    .env("FLUX_SYSTEMCTL", "true")
                    .env("FLUX_SYSTEMD_UNIT_DIR", home.join("units"))
                    .output()
                    .unwrap()
            };
            let o = run(&["profile", "validate", engine]);
            assert!(
                o.status.success(),
                "{what}: {}{}",
                stdout(&o),
                String::from_utf8_lossy(&o.stderr)
            );
            let o = run(&["--json", "snapshots", engine]);
            assert!(
                o.status.success(),
                "{what}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let snaps: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
            let snaps = snaps.as_array().unwrap();
            assert_eq!(snaps.len(), 2, "{what}: {snaps:?}");
            // The newest snapshot, restored, is the files as they were.
            let out = t.path().join(format!("out-{engine}"));
            let o = run(&["restore", engine, "--to", out.to_str().unwrap(), "/srv/src"]);
            assert!(
                o.status.success(),
                "{what}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            for (rel, want) in exp["latest"].as_object().unwrap() {
                let p = out.join("src").join(rel);
                let want = want.as_str().unwrap();
                match want.strip_prefix("link:") {
                    Some(target) => assert_eq!(
                        std::fs::read_link(&p).unwrap(),
                        Path::new(target),
                        "{what}: {rel}"
                    ),
                    None => assert_eq!(sha256(&p), want, "{what}: {rel}"),
                }
            }
            // And the first one, before the change.
            let first = snaps
                .iter()
                .min_by_key(|s| s["started"].as_str().unwrap().to_owned())
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let out1 = t.path().join(format!("first-{engine}"));
            let o = run(&[
                "restore",
                engine,
                "--snapshot",
                &first,
                "--to",
                out1.to_str().unwrap(),
                "/srv/src/docs/a.txt",
            ]);
            assert!(
                o.status.success(),
                "{what}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            assert_eq!(
                sha256(&out1.join("a.txt")),
                exp["first"]["docs/a.txt"].as_str().unwrap(),
                "{what}"
            );
            // This Flux carries on into the old repository. Without pruning: the fixtures'
            // snapshots are a second apart on the day they were made, so once that day is
            // past `keep_within = "1day"`, `keep_daily` would remove one of them.
            let o = run(&["backup", engine, "--no-prune"]);
            assert!(
                o.status.success(),
                "{what}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let o = run(&["--json", "snapshots", engine]);
            let snaps: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
            assert_eq!(snaps.as_array().unwrap().len(), 3, "{what}");
        }
    }
}

/// A run that was cut short leaves the files it brought over in `snapshots/.partial`,
/// and the next run continues it. If only contents changed (nothing added or removed),
/// that next run sends nothing and creates nothing: it must still make the snapshot, not
/// call the backup unchanged and delete `.partial`, the only copy of the new contents.
#[test]
fn rsync_continued_backup_is_never_called_unchanged() {
    if Command::new("rsync").arg("--version").output().is_err() {
        eprintln!("skipping: no rsync");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let src = h.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("report.txt"), "first version").unwrap();
    let target = h.join("drive");
    std::fs::create_dir(&target).unwrap();
    let o = flux(
        h,
        &[
            "profile",
            "create",
            "p",
            "-t",
            target.to_str().unwrap(),
            "-s",
            src.to_str().unwrap(),
            "--engine",
            "rsync",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let f = h.join("config/profiles/p.toml");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("notify = true", "notify = false");
    std::fs::write(&f, text).unwrap();
    let o = flux(h, &["backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // The `snapshots` folder of the backup, wherever below the drive it is.
    fn find_snapshots(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                if e.file_name() == "snapshots" {
                    return Some(p);
                }
                if let Some(found) = find_snapshots(&p) {
                    return Some(found);
                }
            }
        }
        None
    }
    let snapshots = find_snapshots(&target).expect("a snapshots folder");
    let count = |dir: &std::path::Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("20"))
            .count()
    };
    assert_eq!(count(&snapshots), 1);

    // The file changes; a run brings it over and is cut short before the snapshot is
    // made: `.partial/data` holds the new contents (with its time, as rsync leaves it).
    std::fs::write(src.join("report.txt"), "second version, longer").unwrap();
    let rel = src.strip_prefix("/").unwrap();
    let staged = snapshots.join(".partial/data").join(rel);
    std::fs::create_dir_all(&staged).unwrap();
    let o = Command::new("cp")
        .arg("-p")
        .arg(src.join("report.txt"))
        .arg(staged.join("report.txt"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");

    let o = flux(h, &["backup", "p"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        count(&snapshots),
        2,
        "the continued backup became a snapshot: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let mut ids: Vec<String> = std::fs::read_dir(&snapshots)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("20"))
        .collect();
    ids.sort();
    let newest = snapshots
        .join(ids.last().unwrap())
        .join("data")
        .join(rel)
        .join("report.txt");
    assert_eq!(
        std::fs::read_to_string(&newest).unwrap(),
        "second version, longer"
    );
}
