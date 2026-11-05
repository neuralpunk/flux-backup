//! User hooks: shell commands run before and after backups.
//!
//! Each command runs as `sh -c <command>` with stdin closed, and gets these environment
//! variables:
//!
//! | variable | meaning |
//! |---|---|
//! | `FLUX_EVENT` | `pre_backup`, `post_backup` or `on_error` |
//! | `FLUX_PROFILE` / `FLUX_PROFILE_NAME` | profile id / display name |
//! | `FLUX_ENGINE` | `mirror` or `vault` |
//! | `FLUX_SOURCES` | source paths, newline-separated |
//! | `FLUX_TARGET` | target path or `ssh://…` URL |
//! | `FLUX_SNAPSHOT_ID`, `FLUX_SNAPSHOT_PATH` | the new snapshot (post_backup) |
//! | `FLUX_STATUS` | `complete`, `complete_with_errors`, `unchanged` or `failed` |
//! | `FLUX_ERROR` | error message (on_error) |
//!
//! A failing `pre_backup` command aborts the backup. Failures of the other hooks are
//! reported but don't change the backup's outcome.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Default limit for one hook command.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreBackup,
    PostBackup,
    OnError,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreBackup => "pre_backup",
            HookEvent::PostBackup => "post_backup",
            HookEvent::OnError => "on_error",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("{event} hook `{command}` could not start: {source}")]
    Spawn {
        event: &'static str,
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{event} hook `{command}` failed with {status}")]
    Failed {
        event: &'static str,
        command: String,
        status: String,
    },
    #[error("{event} hook `{command}` timed out after {secs}s and was killed")]
    Timeout {
        event: &'static str,
        command: String,
        secs: u64,
    },
}

/// Output of one hook command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRun {
    pub command: String,
    pub success: bool,
    /// Combined stdout+stderr, truncated to 64 KiB.
    pub output: String,
    pub duration_secs: f64,
}

/// Run `commands` in order with `env`. Stops at the first failure and returns it, along
/// with the runs so far.
pub fn run_hooks(
    event: HookEvent,
    commands: &[String],
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> (Vec<HookRun>, Result<(), HookError>) {
    let mut runs = Vec::new();
    for cmd in commands {
        let started = Instant::now();
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .envs(env)
            .env("FLUX_EVENT", event.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Own process group, so a timeout kills the hook's children too (they would
            // otherwise keep our output pipes open).
            .process_group(0)
            .spawn()
        {
            Ok(c) => c,
            Err(source) => {
                return (
                    runs,
                    Err(HookError::Spawn {
                        event: event.as_str(),
                        command: cmd.clone(),
                        source,
                    }),
                );
            }
        };
        // Drain both pipes concurrently so a chatty hook can't block on a full pipe.
        let readers: Vec<_> = [
            child
                .stdout
                .take()
                .map(|o| Box::new(o) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|e| Box::new(e) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|r| {
            std::thread::spawn(move || {
                let mut out = String::new();
                for line in BufReader::new(r).lines().map_while(Result::ok) {
                    if out.len() < 64 << 10 {
                        out.push_str(&line);
                        out.push('\n');
                    }
                }
                out
            })
        })
        .collect();

        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Ok(s),
                Ok(None) if started.elapsed() > timeout => {
                    let pgid = rustix::process::Pid::from_child(&child);
                    let _ =
                        rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break Err(()),
            }
        };
        let output: String = readers.into_iter().filter_map(|h| h.join().ok()).collect();
        let duration_secs = started.elapsed().as_secs_f64();
        let success = matches!(&status, Ok(s) if s.success());
        tracing::info!(
            "{} hook `{cmd}` finished ({}) in {duration_secs:.1}s",
            event.as_str(),
            if success { "ok" } else { "failed" }
        );
        runs.push(HookRun {
            command: cmd.clone(),
            success,
            output,
            duration_secs,
        });
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return (
                    runs,
                    Err(HookError::Failed {
                        event: event.as_str(),
                        command: cmd.clone(),
                        status: s.to_string(),
                    }),
                );
            }
            Err(()) => {
                return (
                    runs,
                    Err(HookError::Timeout {
                        event: event.as_str(),
                        command: cmd.clone(),
                        secs: timeout.as_secs(),
                    }),
                );
            }
        }
    }
    (runs, Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> BTreeMap<String, String> {
        BTreeMap::from([("FLUX_PROFILE".to_owned(), "home".to_owned())])
    }

    #[test]
    fn runs_in_order_with_env() {
        let (runs, r) = run_hooks(
            HookEvent::PreBackup,
            &[
                "echo \"$FLUX_EVENT $FLUX_PROFILE\"".into(),
                "echo second >&2".into(),
            ],
            &env(),
            DEFAULT_TIMEOUT,
        );
        assert!(r.is_ok());
        assert_eq!(runs[0].output, "pre_backup home\n");
        assert_eq!(runs[1].output, "second\n");
    }

    #[test]
    fn stops_at_failure() {
        let (runs, r) = run_hooks(
            HookEvent::PostBackup,
            &["exit 3".into(), "echo never".into()],
            &env(),
            DEFAULT_TIMEOUT,
        );
        assert!(matches!(r, Err(HookError::Failed { .. })));
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn timeout_kills() {
        let t = Instant::now();
        // A compound command leaves a grandchild holding the pipes; it must die too.
        let (_, r) = run_hooks(
            HookEvent::OnError,
            &["sleep 10; echo late".into()],
            &env(),
            Duration::from_millis(200),
        );
        assert!(matches!(r, Err(HookError::Timeout { .. })));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
