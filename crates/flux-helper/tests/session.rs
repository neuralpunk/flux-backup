//! Drives the real `flux-helper` binary through the app's client, without pkexec: the
//! test runs it directly with `FLUX_HOME` pointing at a temporary "system" (so nothing
//! touches /etc), a temporary systemd unit directory, and `true` instead of systemctl.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use flux_service::client::HelperClient;
use flux_service::protocol::Call;
use flux_service::watch::Notice;

const HELPER: &str = env!("CARGO_BIN_EXE_flux-helper");

fn argv(home: &Path) -> Vec<OsString> {
    vec![
        "env".into(),
        format!("FLUX_HOME={}", home.join("sys").display()).into(),
        format!("FLUX_SYSTEMD_UNIT_DIR={}", home.join("units").display()).into(),
        "FLUX_SYSTEMCTL=true".into(),
        "FLUX_LOG=warn".into(),
        HELPER.into(),
    ]
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn system_profile_lifecycle_through_the_helper() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("conf")).unwrap();
    std::fs::write(src.join("conf/app.conf"), "port = 80\n").unwrap();
    std::fs::create_dir_all(tmp.path().join("tgt")).unwrap();
    std::fs::create_dir_all(tmp.path().join("units")).unwrap();

    let notices: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let (exit_tx, exit_rx) = mpsc::channel();
    let client = {
        let notices = notices.clone();
        HelperClient::start(
            &argv(tmp.path()),
            Arc::new(move |n| notices.lock().unwrap().push(n)),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        )
        .expect("helper starts")
    };

    // New system profiles start from system folders.
    let draft = client.call(Call::NewProfile).unwrap();
    assert_eq!(draft["profile"]["name"], "System settings");
    assert_eq!(draft["profile"]["sources"]["paths"][0], "/etc");

    // Create one (pointed at temporary folders) with a daily timer.
    let mut profile = draft["profile"].clone();
    profile["name"] = "Config".into();
    profile["sources"]["paths"] = json!([src]);
    profile["sources"]["exclude"] = json!([]);
    profile["target"] = json!({"kind": "local", "path": tmp.path().join("tgt")});
    let saved = client
        .call(Call::SaveProfile {
            id: "conf".into(),
            profile,
            create: true,
        })
        .unwrap();
    assert_eq!(saved["saved"], true, "{saved}");
    let service =
        std::fs::read_to_string(tmp.path().join("units/flux-backup-conf.service")).unwrap();
    assert!(
        service.contains("--system backup conf --scheduled"),
        "{service}"
    );
    assert!(
        tmp.path().join("units/flux-backup-conf.timer").is_file(),
        "a system timer"
    );

    let list = client.call(Call::Profiles).unwrap();
    assert_eq!(list[0]["id"], "conf");
    assert_eq!(list[0]["scope"], "system");

    // "Back up now" runs in the background (the helper starts itself with --run-backup).
    client.call(Call::BackupNow { id: "conf".into() }).unwrap();
    let mut snaps = Value::Null;
    wait_for("the snapshot", || {
        // "empty" until the first snapshot is complete.
        snaps = client
            .call(Call::Snapshots { id: "conf".into() })
            .unwrap_or(Value::Null);
        snaps.as_array().is_some_and(|a| a.len() == 1)
    });
    let snap = snaps[0]["id"].as_str().unwrap().to_owned();

    let listing = client
        .call(Call::ListDir {
            id: "conf".into(),
            snapshot: snap.clone(),
            path: src.join("conf").display().to_string(),
        })
        .unwrap();
    assert_eq!(listing["entries"][0]["name"], "app.conf");

    // Restore elsewhere.
    let out = tmp.path().join("restored");
    let r = client
        .call(Call::Restore {
            id: "conf".into(),
            snapshot: snap,
            paths: vec![src.join("conf").display().to_string()],
            to: Some(out.display().to_string()),
            overwrite: false,
        })
        .unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    assert_eq!(
        std::fs::read_to_string(out.join("conf/app.conf")).unwrap(),
        "port = 80\n"
    );
    // Relative destinations are refused rather than resolved against the helper's cwd.
    let e = client
        .call(Call::Restore {
            id: "conf".into(),
            snapshot: "latest".into(),
            paths: vec![src.display().to_string()],
            to: Some("relative/dir".into()),
            overwrite: false,
        })
        .unwrap_err();
    assert!(e.message.contains("absolute"), "{e:?}");

    // Errors carry codes like in the app.
    let e = client
        .call(Call::Overview { id: "nope".into() })
        .unwrap_err();
    assert_eq!(e.code, "error");

    // Closing the pipe ends the session.
    client.close();
    exit_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("helper exits when the app lets go");
    assert!(!client.is_alive());
    assert_eq!(client.call(Call::Profiles).unwrap_err().code, "locked");
}

/// Anything outside the protocol gets a `bad_request` reply and changes nothing.
#[test]
fn refuses_requests_outside_the_protocol() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new(HELPER)
        .env("FLUX_HOME", tmp.path().join("sys"))
        .env("FLUX_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let ready: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["type"], "ready");

    let long = format!("{{\"id\":9,\"pad\":\"{}\"}}\n", "x".repeat(5 << 20));
    for (req, id) in [
        (
            "{\"id\":1,\"call\":{\"method\":\"exec\",\"params\":{\"cmd\":\"id\"}}}\n",
            1,
        ),
        ("not json\n", 0),
        (long.as_str(), 0),
        (
            "{\"id\":2,\"call\":{\"method\":\"delete_profile\",\"params\":{\"id\":\"../../etc\"}}}\n",
            2,
        ),
    ] {
        stdin.write_all(req.as_bytes()).unwrap();
        line.clear();
        stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "reply", "{line}");
        assert_eq!(v["id"], id, "{line}");
        assert!(v["err"]["code"].is_string(), "{line}");
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
