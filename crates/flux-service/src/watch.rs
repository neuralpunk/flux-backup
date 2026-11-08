//! Relays running jobs: every job's status socket (manual, scheduled, or started from a
//! terminal) becomes [`Notice::Job`]s, and a finished job sends [`Notice::Changed`] so
//! views reload.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use flux_core::progress::ProgressSnapshot;
use flux_core::{Paths, ProfileId};
use flux_runner::jobs::{self, Client, Message};

/// Something the frontend should hear about without asking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    /// A message from a running backup.
    Job { profile: String, message: Message },
    /// A profile's snapshots changed (a backup finished).
    Changed { profile: String },
    /// Progress of a restore started through this service.
    Restore {
        profile: String,
        progress: ProgressSnapshot,
    },
}

impl Notice {
    pub fn profile_mut(&mut self) -> &mut String {
        match self {
            Notice::Job { profile, .. }
            | Notice::Changed { profile }
            | Notice::Restore { profile, .. } => profile,
        }
    }
}

pub type Sink = Arc<dyn Fn(Notice) + Send + Sync>;

/// Watch `paths`' jobs for the rest of the process, sending what happens to `sink`.
pub fn start(paths: Paths, sink: Sink) {
    let watching: Arc<Mutex<HashSet<String>>> = Arc::default();
    std::thread::spawn(move || {
        // Woken as soon as a job file appears (inotify), with a slow re-check as a safety
        // net; without inotify, the old polling.
        let waker = Waker::new(&jobs::jobs_dir(&paths));
        loop {
            for job in jobs::running(&paths) {
                if !watching
                    .lock()
                    .expect("watch set")
                    .insert(job.profile.clone())
                {
                    continue;
                }
                let (sink, paths, watching) = (sink.clone(), paths.clone(), watching.clone());
                std::thread::spawn(move || {
                    follow(&*sink, &paths, &job.profile);
                    watching.lock().expect("watch set").remove(&job.profile);
                });
            }
            match &waker {
                Some(w) => w.wait(Duration::from_secs(5)),
                None => std::thread::sleep(Duration::from_millis(700)),
            }
        }
    });
}

/// Wakes up when anything changes in the jobs folder (a job starts or ends).
struct Waker {
    fd: rustix::fd::OwnedFd,
}

impl Waker {
    fn new(dir: &std::path::Path) -> Option<Self> {
        use rustix::fs::inotify;
        std::fs::create_dir_all(dir).ok()?;
        let fd =
            inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).ok()?;
        inotify::add_watch(
            &fd,
            dir,
            inotify::WatchFlags::CREATE
                | inotify::WatchFlags::MOVED_TO
                | inotify::WatchFlags::CLOSE_WRITE
                | inotify::WatchFlags::DELETE,
        )
        .ok()?;
        Some(Self { fd })
    }

    /// Wait for a change, or `timeout`, then drain what's queued.
    fn wait(&self, timeout: Duration) {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let t = Timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        let _ = poll(&mut fds, Some(&t));
        let mut buf = [0u8; 4096];
        while matches!(rustix::io::read(&self.fd, &mut buf), Ok(n) if n > 0) {}
    }
}

fn follow(sink: &(dyn Fn(Notice) + Send + Sync), paths: &Paths, profile: &str) {
    let Ok(id) = ProfileId::new(profile) else {
        return;
    };
    let Ok(mut client) = Client::connect(paths, &id) else {
        return;
    };
    while let Ok(Some(message)) = client.next_message() {
        let done = matches!(message, Message::Finished { .. });
        sink(Notice::Job {
            profile: profile.to_owned(),
            message,
        });
        if done {
            sink(Notice::Changed {
                profile: profile.to_owned(),
            });
            break;
        }
    }
}

#[cfg(test)]
mod waker_tests {
    use super::*;

    #[test]
    fn wakes_when_a_job_file_appears() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = dir.path().join("jobs");
        let w = Waker::new(&jobs).expect("inotify");
        let started = std::time::Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                flux_core::store::write_atomic(&jobs.join("p.json"), b"{}", 0o600).unwrap();
            });
            w.wait(Duration::from_secs(5));
        });
        let waited = started.elapsed();
        assert!(waited < Duration::from_secs(2), "woke after {waited:?}");
        // An atomic write is several events (create, close, rename); once they're drained,
        // a quiet folder lets the wait run to its timeout.
        w.wait(Duration::ZERO);
        let t = std::time::Instant::now();
        w.wait(Duration::from_millis(200));
        assert!(t.elapsed() >= Duration::from_millis(150));
    }
}
