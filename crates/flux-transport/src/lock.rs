//! Repository locks that work wherever the repository is reached from: a small JSON file
//! per holder in a `locks/` directory, refreshed while held. The same files are written
//! over SFTP and on a local disk, so a repository used both ways (a NAS mounted on one
//! computer and reached over SSH from another) has one lock.
//!
//! A lock is ignored as stale when it hasn't been refreshed for [`STALE_AFTER`] (a crashed
//! or disconnected job), or when it was taken on this computer by a process that no longer
//! exists. Acquiring writes the lock first and checks again afterwards, so two clients
//! racing for an exclusive lock can't both win.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ssh::SftpPool;

pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub host: String,
    pub user: String,
    pub pid: u32,
    pub exclusive: bool,
    pub time: Timestamp,
    /// Which computer took it (see [`machine_token`]). Two computers can have the same
    /// host name (two fresh installs both called "fedora"), so the name alone doesn't say
    /// that a lock's process id is one of ours. Locks written before 1.0 have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl LockInfo {
    /// Not refreshed for too long, or taken on this computer by a process that's gone.
    pub fn is_stale(&self, now: Timestamp) -> bool {
        now.duration_since(self.time).as_secs() > STALE_AFTER.as_secs() as i64
            || (self.is_from_this_computer() && !process_exists(self.pid))
    }

    /// Taken on this very computer (not just one with the same name), so its process id
    /// means something here.
    fn is_from_this_computer(&self) -> bool {
        self.machine.is_some()
            && self.machine == machine_token()
            && self.host == flux_core::sys::hostname()
    }

    /// "alice@laptop, process 4242, since 2026-11-09 10:15" (for messages).
    pub fn describe(&self) -> String {
        format!(
            "{}@{}, process {}, since {}",
            self.user,
            self.host,
            self.pid,
            self.time.strftime("%Y-%m-%d %H:%M UTC")
        )
    }
}

/// A token that is the same for every lock taken on this computer and different on any
/// other: derived from `/etc/machine-id` (never the id itself, which isn't meant to leave
/// the machine). `None` when the computer has no machine id.
pub fn machine_token() -> Option<String> {
    static TOKEN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    TOKEN
        .get_or_init(|| {
            let id = std::fs::read_to_string("/etc/machine-id").ok()?;
            let id = id.trim();
            if id.is_empty() {
                return None;
            }
            let key = blake3::derive_key("flux-backup 1.0 lock machine token", id.as_bytes());
            Some(key[..12].iter().map(|b| format!("{b:02x}")).collect())
        })
        .clone()
}

fn process_exists(pid: u32) -> bool {
    pid == std::process::id() || PathBuf::from(format!("/proc/{pid}")).exists()
}

/// Where lock files live: a directory on an SFTP server or on a local disk. Paths are
/// strings, as the server sees them (absolute on a local disk).
pub trait LockStore: Send + Sync {
    fn mkdir_p(&self, dir: &str) -> io::Result<()>;
    /// File names in `dir` (a missing directory has none).
    fn names(&self, dir: &str) -> io::Result<Vec<String>>;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// Write `data` to `dir/name` atomically (readers see the old or the new content).
    fn write_atomic(&self, dir: &str, name: &str, data: &[u8]) -> io::Result<()>;
    fn remove(&self, path: &str) -> io::Result<()>;
}

impl LockStore for SftpPool {
    fn mkdir_p(&self, dir: &str) -> io::Result<()> {
        self.with(|s| s.mkdir_p(dir, 0o700))
    }

    fn names(&self, dir: &str) -> io::Result<Vec<String>> {
        match self.with(|s| s.readdir(dir)) {
            Ok(entries) => Ok(entries.into_iter().map(|(n, _)| n).collect()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.with(|s| s.read_file(path))
    }

    fn write_atomic(&self, dir: &str, name: &str, data: &[u8]) -> io::Result<()> {
        let tmp = format!("{dir}/.{name}.tmp");
        let path = format!("{dir}/{name}");
        self.with(|s| {
            s.write_new(&tmp, data, 0o600)?;
            s.rename(&tmp, &path)
        })
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.with(|s| s.remove(path))
    }
}

/// Lock files on a local (or locally mounted) disk.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalStore;

impl LockStore for LocalStore {
    fn mkdir_p(&self, dir: &str) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }

    fn names(&self, dir: &str) -> io::Result<Vec<String>> {
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write_atomic(&self, dir: &str, name: &str, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = format!("{dir}/.{name}.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, format!("{dir}/{name}"))
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        std::fs::remove_file(path)
    }
}

/// A held lock. Released (file removed) on drop.
pub struct RemoteLock {
    store: Arc<dyn LockStore>,
    path: String,
    stop: Arc<AtomicBool>,
    refresher: Option<JoinHandle<()>>,
}

impl Drop for RemoteLock {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.refresher.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        let _ = self.store.remove(&self.path);
    }
}

/// Every lock file in `dir`: (file name, the lock, whether it's stale). Unreadable files
/// (a lock being written) are left out.
pub fn list(store: &dyn LockStore, dir: &str) -> io::Result<Vec<(String, LockInfo, bool)>> {
    let now = Timestamp::now();
    let mut out = Vec::new();
    for name in store.names(dir)? {
        if name.starts_with('.') {
            continue;
        }
        let Ok(raw) = store.read(&format!("{dir}/{name}")) else {
            continue; // vanished meanwhile
        };
        match serde_json::from_slice::<LockInfo>(&raw) {
            Ok(l) => {
                let stale = l.is_stale(now);
                out.push((name, l, stale));
            }
            Err(_) => tracing::warn!("ignoring unreadable lock {dir}/{name}"),
        }
    }
    Ok(out)
}

/// The locks in `dir` that are held (not stale).
pub fn holders(store: &dyn LockStore, dir: &str) -> io::Result<Vec<LockInfo>> {
    Ok(list(store, dir)?
        .into_iter()
        .filter(|(_, _, stale)| !stale)
        .map(|(_, l, _)| l)
        .collect())
}

fn live_locks(store: &dyn LockStore, dir: &str, except: Option<&str>) -> io::Result<Vec<LockInfo>> {
    Ok(list(store, dir)?
        .into_iter()
        .filter(|(name, l, stale)| {
            if *stale {
                tracing::info!("ignoring stale lock {dir}/{name} ({})", l.describe());
            }
            !stale && Some(name.as_str()) != except
        })
        .map(|(_, l, _)| l)
        .collect())
}

fn conflicts(locks: &[LockInfo], exclusive: bool) -> bool {
    if exclusive {
        !locks.is_empty()
    } else {
        locks.iter().any(|l| l.exclusive)
    }
}

fn write_lock(store: &dyn LockStore, dir: &str, name: &str, l: &LockInfo) -> io::Result<()> {
    let json = serde_json::to_vec(l).map_err(io::Error::other)?;
    store.write_atomic(dir, name, &json)
}

/// Try to take a lock in `dir`: the lock, or the locks in the way.
pub fn acquire_in(
    store: Arc<dyn LockStore>,
    dir: &str,
    exclusive: bool,
) -> io::Result<Result<RemoteLock, Vec<LockInfo>>> {
    store.mkdir_p(dir)?;
    let held = live_locks(store.as_ref(), dir, None)?;
    if conflicts(&held, exclusive) {
        return Ok(Err(held));
    }
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).map_err(|e| io::Error::other(e.to_string()))?;
    let name: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let path = format!("{dir}/{name}");
    let lock = LockInfo {
        host: flux_core::sys::hostname(),
        user: flux_core::sys::username(),
        pid: std::process::id(),
        exclusive,
        time: Timestamp::now(),
        machine: machine_token(),
    };
    write_lock(store.as_ref(), dir, &name, &lock)?;
    // Re-check: someone may have written a conflicting lock at the same time.
    let held = live_locks(store.as_ref(), dir, Some(&name))?;
    if conflicts(&held, exclusive) {
        let _ = store.remove(&path);
        return Ok(Err(held));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let refresher = {
        let (store, stop, dir, name) = (store.clone(), stop.clone(), dir.to_owned(), name.clone());
        std::thread::spawn(move || {
            while !stop.load(Relaxed) {
                std::thread::park_timeout(REFRESH_EVERY);
                if stop.load(Relaxed) {
                    break;
                }
                let fresh = LockInfo {
                    time: Timestamp::now(),
                    ..lock.clone()
                };
                if let Err(e) = write_lock(store.as_ref(), &dir, &name, &fresh) {
                    tracing::warn!("cannot refresh repository lock: {e}");
                }
            }
        })
    };
    Ok(Ok(RemoteLock {
        store,
        path,
        stop,
        refresher: Some(refresher),
    }))
}

/// Try to take a lock in `dir` on an SFTP server. `Ok(None)` if a conflicting lock is held.
pub fn acquire(pool: Arc<SftpPool>, dir: &str, exclusive: bool) -> io::Result<Option<RemoteLock>> {
    Ok(acquire_in(pool, dir, exclusive)?.ok())
}

/// Remove lock files from `dir`: the stale ones, or (`force`) every one. For when a job
/// on another computer died and its lock would otherwise block for [`STALE_AFTER`].
/// Returns the locks removed.
pub fn clear(store: &dyn LockStore, dir: &str, force: bool) -> io::Result<Vec<LockInfo>> {
    let mut removed = Vec::new();
    for (name, l, stale) in list(store, dir)? {
        if stale || force {
            match store.remove(&format!("{dir}/{name}")) {
                Ok(()) => removed.push(l),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::local_target;

    #[test]
    fn shared_and_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let Some(target) = local_target(t.path()) else {
            return;
        };
        let pool = Arc::new(SftpPool::new(target, 2));
        let dir = t.path().join("locks").display().to_string();
        let a = acquire(pool.clone(), &dir, false).unwrap().unwrap();
        let b = acquire(pool.clone(), &dir, false).unwrap().unwrap();
        assert!(acquire(pool.clone(), &dir, true).unwrap().is_none());
        drop((a, b));
        let x = acquire(pool.clone(), &dir, true).unwrap().unwrap();
        assert!(acquire(pool.clone(), &dir, false).unwrap().is_none());
        drop(x);
        assert!(
            std::fs::read_dir(t.path().join("locks"))
                .unwrap()
                .next()
                .is_none(),
            "released locks are removed"
        );

        // A stale lock (crashed job) doesn't block.
        let old = LockInfo {
            host: "h".into(),
            user: "u".into(),
            pid: 1,
            exclusive: true,
            time: Timestamp::from_second(1_000_000_000).unwrap(),
            machine: None,
        };
        std::fs::write(
            t.path().join("locks/deadbeef"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        assert!(acquire(pool, &dir, true).unwrap().is_some());
    }

    /// A process id that isn't running (the highest possible ids are free in practice).
    fn dead_pid() -> u32 {
        (0..1000)
            .map(|i| 4_194_000 - i)
            .find(|p| !process_exists(*p))
            .expect("a free process id")
    }

    fn write(dir: &std::path::Path, name: &str, l: &LockInfo) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), serde_json::to_vec(l).unwrap()).unwrap();
    }

    #[test]
    fn local_and_remote_see_the_same_lock() {
        let t = tempfile::tempdir().unwrap();
        let Some(target) = local_target(t.path()) else {
            return;
        };
        let pool = Arc::new(SftpPool::new(target, 2));
        let dir = t.path().join("locks").display().to_string();
        let local: Arc<dyn LockStore> = Arc::new(LocalStore);

        // Taken on the disk: the SFTP side sees it, and is told who holds it.
        let held = acquire_in(local.clone(), &dir, true).unwrap().unwrap();
        let busy = acquire_in(pool.clone(), &dir, false)
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].pid, std::process::id());
        assert!(busy[0].describe().contains(&flux_core::sys::hostname()));
        drop(held);

        // Taken over SFTP: the disk side waits for it.
        let held = acquire_in(pool.clone(), &dir, false).unwrap().unwrap();
        assert!(acquire_in(local.clone(), &dir, true).unwrap().is_err());
        assert!(acquire_in(local.clone(), &dir, false).unwrap().is_ok());
        drop(held);
    }

    #[test]
    fn dead_processes_on_this_computer_hold_nothing() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("locks");
        let now = Timestamp::now();
        if machine_token().is_none() {
            return; // no machine id here: only a lock's age says it's stale
        }
        let dead = LockInfo {
            host: flux_core::sys::hostname(),
            user: "me".into(),
            pid: dead_pid(),
            exclusive: true,
            time: now,
            machine: machine_token(),
        };
        // Another computer with the same name (two fresh installs), or a lock that doesn't
        // say which computer took it: the process id means nothing here, so it holds.
        let namesake = LockInfo {
            machine: Some("0123456789abcdef01234567".into()),
            ..dead.clone()
        };
        let unsaid = LockInfo {
            machine: None,
            ..dead.clone()
        };
        assert!(!namesake.is_stale(now) && !unsaid.is_stale(now));
        // A fresh lock from another computer can't be checked: it holds.
        let elsewhere = LockInfo {
            host: "far-away-host".into(),
            pid: dead.pid,
            ..dead.clone()
        };
        write(&dir, "aaaa", &dead);
        let d = dir.display().to_string();
        assert!(dead.is_stale(now));
        assert!(!elsewhere.is_stale(now));
        let local: Arc<dyn LockStore> = Arc::new(LocalStore);
        let got = acquire_in(local.clone(), &d, true).unwrap();
        assert!(got.is_ok(), "a dead local process's lock doesn't block");
        drop(got);

        write(&dir, "bbbb", &elsewhere);
        let busy = acquire_in(local.clone(), &d, false).unwrap().err().unwrap();
        assert_eq!(busy, vec![elsewhere.clone()]);
        assert_eq!(
            holders(local.as_ref(), &d).unwrap(),
            vec![elsewhere.clone()]
        );

        // Clearing removes what's stale; forcing removes the rest.
        write(&dir, "aaaa", &dead);
        assert_eq!(clear(local.as_ref(), &d, false).unwrap(), vec![dead]);
        assert_eq!(list(local.as_ref(), &d).unwrap().len(), 1);
        assert_eq!(clear(local.as_ref(), &d, true).unwrap(), vec![elsewhere]);
        assert!(list(local.as_ref(), &d).unwrap().is_empty());
    }
}
