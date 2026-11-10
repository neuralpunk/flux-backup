//! Test support: an `ssh` stand-in that runs everything locally, so SFTP, remote commands
//! and rsync-over-ssh can be tested without an SSH server.

use std::path::{Path, PathBuf};

use crate::ssh::SshTarget;

pub fn sftp_server() -> Option<&'static str> {
    [
        "/usr/libexec/openssh/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
}

/// Write a fake `ssh` into `dir`. It accepts ssh's option syntax, then runs the SFTP
/// server for `-s … sftp` or the remote command through `sh -c` locally.
pub fn fake_ssh(dir: &Path) -> Option<PathBuf> {
    let server = sftp_server()?;
    let path = dir.join("fake-ssh");
    let script = format!(
        r#"#!/bin/sh
subsystem=0
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i|-l|-O) shift 2 ;;
    -s) subsystem=1; shift ;;
    --) shift; break ;;
    -*) shift ;;
    *) shift; break ;;   # the destination
  esac
done
[ "$1" = "--" ] && shift
if [ $subsystem = 1 ]; then exec {server}; fi
exec sh -c "$*"
"#
    );
    // Never hold the executable open for writing in this (multithreaded) process: a child
    // forked meanwhile by another test would share that descriptor until its exec, and
    // running the script then fails with "Text file busy". Write the text elsewhere and
    // let `install`, a process of its own that has exited by the time we run it, create
    // the executable.
    let text = dir.join("fake-ssh.txt");
    std::fs::write(&text, script).ok()?;
    let ok = std::process::Command::new("install")
        .args(["-m", "0755"])
        .arg(&text)
        .arg(&path)
        .status()
        .ok()?
        .success();
    ok.then_some(path)
}

/// An [`SshTarget`] for "localhost" through [`fake_ssh`] (no connection sharing).
pub fn local_target(dir: &Path) -> Option<SshTarget> {
    let program = fake_ssh(dir)?.display().to_string();
    Some(SshTarget {
        host: "localhost".into(),
        user: None,
        port: None,
        identity: None,
        options: Vec::new(),
        control_dir: None,
        batch: true,
        program,
    })
}

/// Create an executable script at `path` (a fake `shutdown`, `nmcli`, `flux`… for tests),
/// without this multithreaded process ever holding it open for writing: a child forked
/// meanwhile by another test would share the descriptor until its exec, and running the
/// script would fail with "Text file busy". `install`, a process of its own, creates it.
pub fn executable(path: &Path, text: &str) -> bool {
    let src = path.with_extension("script-text");
    std::fs::write(&src, text).is_ok()
        && std::process::Command::new("install")
            .args(["-m", "0755"])
            .arg(&src)
            .arg(path)
            .status()
            .is_ok_and(|s| s.success())
}
