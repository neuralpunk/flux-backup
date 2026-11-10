//! Running things on the remote host with the system `ssh` client.
//!
//! Using OpenSSH (not a library) means `~/.ssh/config`, agents, ProxyJump, hardware keys
//! and known_hosts all work as they do in a terminal. Connections are shared through
//! `ControlMaster`, so the many short commands and SFTP sessions of a backup cost one
//! handshake.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use flux_core::profile::Target;

use crate::sftp::Sftp;
use flux_core::sync::Locked;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("cannot run ssh: {0}")]
    Spawn(std::io::Error),
    #[error("ssh to {host} failed: {message}")]
    Failed { host: String, message: String },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// How to reach one SSH target.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity: Option<PathBuf>,
    pub options: Vec<String>,
    /// Directory for ControlMaster sockets; `None` disables connection sharing.
    pub control_dir: Option<PathBuf>,
    /// Never prompt (scheduled runs): unknown host keys and password logins fail fast.
    pub batch: bool,
    /// The ssh program ([`ssh_program`] by default).
    pub program: String,
}

/// The `ssh` program to run. `FLUX_SSH` overrides it (tests use a local stand-in).
pub fn ssh_program() -> String {
    std::env::var("FLUX_SSH").unwrap_or_else(|_| "ssh".into())
}

/// Quote for a POSIX shell: `it's` → `'it'\''s'`.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:@=+,".contains(&b))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl SshTarget {
    /// Build from a profile target; `None` for local targets.
    pub fn from_target(t: &Target, runtime_dir: &Path, batch: bool) -> Option<Self> {
        let Target::Ssh {
            host,
            user,
            port,
            identity_file,
            ssh_options,
            ..
        } = t
        else {
            return None;
        };
        Some(Self {
            host: host.clone(),
            user: user.clone(),
            port: *port,
            identity: identity_file.clone(),
            options: ssh_options.clone(),
            control_dir: Some(control_dir(runtime_dir)),
            batch,
            program: ssh_program(),
        })
    }

    pub fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// Options before the destination, shared by ssh, the SFTP subsystem and rsync's `-e`.
    pub fn options_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        let mut o = |s: String| {
            a.push("-o".to_owned());
            a.push(s);
        };
        if self.batch {
            o("BatchMode=yes".into());
        }
        o("ServerAliveInterval=30".into());
        o("ServerAliveCountMax=4".into());
        if let Some(dir) = &self.control_dir
            && std::fs::create_dir_all(dir).is_ok()
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            o("ControlMaster=auto".into());
            o(format!("ControlPath={}/%C", dir.display()));
            o("ControlPersist=60".into());
        }
        for x in &self.options {
            o(x.clone());
        }
        if let Some(p) = self.port {
            a.push("-p".into());
            a.push(p.to_string());
        }
        if let Some(i) = &self.identity {
            a.push("-i".into());
            a.push(i.display().to_string());
            a.push("-o".into());
            a.push("IdentitiesOnly=yes".into());
        }
        a
    }

    /// `ssh <opts> <dest> -- <remote_command>`.
    pub fn command(&self, remote_command: &str) -> Command {
        let mut c = Command::new(&self.program);
        c.args(self.options_args())
            .arg(self.destination())
            .arg("--")
            .arg(remote_command);
        c
    }

    /// Run a remote shell command; returns its stdout.
    pub fn run(&self, remote_command: &str) -> Result<String, SshError> {
        let out = self
            .command(remote_command)
            .stdin(Stdio::null())
            .output()
            .map_err(SshError::Spawn)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            return Err(SshError::Failed {
                host: self.destination(),
                message: if err.is_empty() {
                    format!("`{remote_command}` exited with {}", out.status)
                } else {
                    err
                },
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Open an SFTP session (`ssh -s <dest> sftp`).
    pub fn sftp(&self) -> Result<Sftp, SshError> {
        let child = Command::new(&self.program)
            .args(self.options_args())
            .arg("-s")
            .arg(self.destination())
            .arg("sftp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(SshError::Spawn)?;
        Sftp::start(child).map_err(|e| SshError::Failed {
            host: self.destination(),
            message: e.to_string(),
        })
    }

    /// The `-e` argument for rsync: the same ssh with the same options.
    pub fn rsync_rsh(&self) -> String {
        std::iter::once(self.program.clone())
            .chain(self.options_args())
            .map(|a| shell_quote(&a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Close the shared connection (best-effort).
    pub fn close_master(&self) {
        if self.control_dir.is_some() {
            let _ = Command::new(&self.program)
                .args(self.options_args())
                .args(["-O", "exit"])
                .arg(self.destination())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// ControlMaster sockets live in the runtime dir; `%C` adds 40 characters, and socket
/// paths must stay under ~104 bytes, so fall back to a short private dir when needed.
fn control_dir(runtime_dir: &Path) -> PathBuf {
    let preferred = runtime_dir.join("ssh");
    if preferred.as_os_str().len() + 41 < 100 {
        return preferred;
    }
    std::env::temp_dir().join(format!("flux-ssh-{}", flux_core::sys::euid()))
}

/// Several SFTP sessions to one target, for parallel workers. Sessions are opened on
/// demand and multiplexed over the shared ssh connection.
pub struct SftpPool {
    target: SshTarget,
    sessions: Vec<Mutex<Option<Sftp>>>,
}

impl SftpPool {
    pub fn new(target: SshTarget, size: usize) -> Self {
        Self {
            target,
            sessions: (0..size.max(1)).map(|_| Mutex::new(None)).collect(),
        }
    }

    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    /// Run `f` on a free session (or wait for the first one).
    pub fn with<T>(&self, f: impl FnOnce(&mut Sftp) -> std::io::Result<T>) -> std::io::Result<T> {
        let mut guard = self
            .sessions
            .iter()
            .find_map(|m| m.try_lock().ok())
            .unwrap_or_else(|| self.sessions[0].locked());
        if guard.is_none() {
            *guard = Some(self.target.sftp().map_err(std::io::Error::other)?);
        }
        let s = guard.as_mut().expect("session just opened");
        let r = f(s);
        if let Err(e) = &r
            && matches!(
                e.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::InvalidData
            )
        {
            // The session died; open a fresh one next time.
            *guard = None;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/srv/backup"), "/srv/backup");
        assert_eq!(shell_quote("my dir"), "'my dir'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn args() {
        let t = SshTarget {
            host: "nas".into(),
            user: Some("me".into()),
            port: Some(2222),
            identity: Some("/k/id".into()),
            options: vec!["Compression=yes".into()],
            control_dir: None,
            batch: true,
            program: "ssh".into(),
        };
        assert_eq!(t.destination(), "me@nas");
        let a = t.options_args().join(" ");
        assert!(a.contains("-o BatchMode=yes"));
        assert!(a.contains("-o Compression=yes"));
        assert!(a.contains("-p 2222"));
        assert!(a.contains("-i /k/id -o IdentitiesOnly=yes"));
        assert!(!a.contains("ControlMaster"));
    }
}
