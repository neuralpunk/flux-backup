//! Repository locks for remote targets, where `flock` doesn't reach. A lock is a small
//! JSON file in a `locks/` directory, refreshed while held. A lock that hasn't been
//! refreshed for [`STALE_AFTER`] belongs to a crashed or disconnected job and is ignored.
//! Acquiring writes the lock first and checks again afterwards, so two clients racing for
//! an exclusive lock can't both win.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ssh::SftpPool;

pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub host: String,
    pub user: String,
    pub pid: u32,
    pub exclusive: bool,
    pub time: Timestamp,
}

impl LockInfo {
    fn is_stale(&self, now: Timestamp) -> bool {
        now.duration_since(self.time).as_secs() > STALE_AFTER.as_secs() as i64
    }
}

/// A held remote lock. Released (file removed) on drop.
pub struct RemoteLock {
    pool: Arc<SftpPool>,
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
        let _ = self.pool.with(|s| s.remove(&self.path));
    }
}

fn live_locks(pool: &SftpPool, dir: &str, except: Option<&str>) -> io::Result<Vec<LockInfo>> {
    let now = Timestamp::now();
    let names = pool.with(|s| s.readdir(dir))?;
    let mut out = Vec::new();
    for (name, _) in names {
        if Some(name.as_str()) == except || name.starts_with('.') {
            continue;
        }
        let path = format!("{dir}/{name}");
        let Ok(raw) = pool.with(|s| s.read_file(&path)) else {
            continue;
        }; // vanished meanwhile
        match serde_json::from_slice::<LockInfo>(&raw) {
            Ok(l) if !l.is_stale(now) => out.push(l),
            Ok(_) => tracing::info!("ignoring stale lock {path}"),
            Err(_) => tracing::warn!("ignoring unreadable lock {path}"),
        }
    }
    Ok(out)
}

fn conflicts(locks: &[LockInfo], exclusive: bool) -> bool {
    if exclusive {
        !locks.is_empty()
    } else {
        locks.iter().any(|l| l.exclusive)
    }
}

fn write_lock(pool: &SftpPool, dir: &str, name: &str, l: &LockInfo) -> io::Result<()> {
    let json = serde_json::to_vec(l).map_err(io::Error::other)?;
    let tmp = format!("{dir}/.{name}.tmp");
    let path = format!("{dir}/{name}");
    pool.with(|s| {
        s.write_new(&tmp, &json, 0o600)?;
        s.rename(&tmp, &path)
    })
}

/// Try to take a lock in `dir`. `Ok(None)` if a conflicting lock is held.
pub fn acquire(pool: Arc<SftpPool>, dir: &str, exclusive: bool) -> io::Result<Option<RemoteLock>> {
    pool.with(|s| s.mkdir_p(dir, 0o700))?;
    if conflicts(&live_locks(&pool, dir, None)?, exclusive) {
        return Ok(None);
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
    };
    write_lock(&pool, dir, &name, &lock)?;
    // Re-check: someone may have written a conflicting lock at the same time.
    if conflicts(&live_locks(&pool, dir, Some(&name))?, exclusive) {
        let _ = pool.with(|s| s.remove(&path));
        return Ok(None);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let refresher = {
        let (pool, stop, dir, name) = (pool.clone(), stop.clone(), dir.to_owned(), name.clone());
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
                if let Err(e) = write_lock(&pool, &dir, &name, &fresh) {
                    tracing::warn!("cannot refresh repository lock: {e}");
                }
            }
        })
    };
    Ok(Some(RemoteLock {
        pool,
        path,
        stop,
        refresher: Some(refresher),
    }))
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
        };
        std::fs::write(
            t.path().join("locks/deadbeef"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        assert!(acquire(pool, &dir, true).unwrap().is_some());
    }
}
