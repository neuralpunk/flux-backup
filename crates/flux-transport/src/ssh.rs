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

/// Files up to this size are fetched in batches by [`SmallFiles`].
pub const SMALL_FILE: u64 = 256 << 10;
/// Files fetched together.
const SMALL_BATCH: usize = 32;
/// Fetched files held until they are asked for (others are fetched again if they are).
const SMALL_HELD: usize = 1024;

/// The small files of a restore from a server, fetched a batch at a time as they are
/// asked for: asking for one brings it and the ones after it in two round trips, where
/// each alone costs three or four. A restore of a million small files waits hours for
/// the server otherwise, whatever the line's speed.
pub struct SmallFiles {
    pool: std::sync::Arc<SftpPool>,
    /// (path on the server, size), sorted by path.
    files: Vec<(Vec<u8>, u64)>,
    state: Mutex<SmallState>,
    arrived: std::sync::Condvar,
}

#[derive(Default)]
struct SmallState {
    got: std::collections::HashMap<usize, std::io::Result<Vec<u8>>>,
    /// The order they came in (the oldest go first when too many are held).
    order: std::collections::VecDeque<usize>,
    /// Being fetched now, by some thread.
    fetching: std::collections::HashSet<usize>,
}

impl SmallFiles {
    /// `files`: path on the server and size of each file that will be asked for. Those
    /// over [`SMALL_FILE`], and empty ones, are left out (ask for them the plain way).
    pub fn new(
        pool: std::sync::Arc<SftpPool>,
        files: impl IntoIterator<Item = (Vec<u8>, u64)>,
    ) -> Self {
        let mut files: Vec<(Vec<u8>, u64)> = files
            .into_iter()
            .filter(|(_, size)| (1..=SMALL_FILE).contains(size))
            .collect();
        files.sort();
        files.dedup_by(|a, b| a.0 == b.0);
        Self {
            pool,
            files,
            state: Mutex::new(SmallState::default()),
            arrived: std::sync::Condvar::new(),
        }
    }

    /// The contents of `path`, if it is one of the small files: fetched with the ones
    /// after it, or waiting here from an earlier fetch. `None`: not one of them.
    pub fn get(&self, path: &[u8]) -> Option<std::io::Result<Vec<u8>>> {
        let idx = self
            .files
            .binary_search_by(|(p, _)| p.as_slice().cmp(path))
            .ok()?;
        let mut st = self.state.locked();
        loop {
            if let Some(r) = st.got.remove(&idx) {
                return Some(r);
            }
            if st.fetching.contains(&idx) {
                st = self
                    .arrived
                    .wait(st)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            // This one and the next ones nobody has or is fetching.
            let batch: Vec<usize> = (idx..self.files.len())
                .filter(|i| !st.got.contains_key(i) && !st.fetching.contains(i))
                .take(SMALL_BATCH)
                .collect();
            st.fetching.extend(batch.iter().copied());
            drop(st);
            let reqs: Vec<crate::sftp::ReadReq> = batch
                .iter()
                .map(|&i| crate::sftp::ReadReq {
                    path: self.files[i].0.clone(),
                    offset: 0,
                    len: Some(self.files[i].1),
                })
                .collect();
            let fetched = self.pool.with(|s| Ok(s.read_many(&reqs)));
            st = self.state.locked();
            match fetched {
                Ok(results) => {
                    for (&i, r) in batch.iter().zip(results) {
                        st.fetching.remove(&i);
                        st.got.insert(i, r);
                        st.order.push_back(i);
                    }
                }
                Err(e) => {
                    for &i in &batch {
                        st.fetching.remove(&i);
                    }
                    st.got
                        .insert(idx, Err(std::io::Error::new(e.kind(), e.to_string())));
                    st.order.push_back(idx);
                }
            }
            while st.order.len() > SMALL_HELD {
                if let Some(old) = st.order.pop_front()
                    && old != idx
                {
                    st.got.remove(&old);
                }
            }
            self.arrived.notify_all();
        }
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
