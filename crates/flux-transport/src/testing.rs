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
    std::fs::write(&path, script).ok()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
    Some(path)
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
