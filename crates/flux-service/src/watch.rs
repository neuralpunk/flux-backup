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
            std::thread::sleep(Duration::from_millis(700));
        }
    });
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
