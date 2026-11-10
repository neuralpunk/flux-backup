//! The app's side of the helper: start `pkexec flux-helper`, wait for it to be authorized,
//! then send [`Call`]s and receive replies and notices.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::Value;

use crate::protocol::{Call, Output, PROTOCOL_VERSION, Request};
use crate::watch::Sink;
use crate::{ApiError, Result};
use flux_core::sync::Locked;

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value>>>>>;

/// Where the helper is installed: `$FLUX_HELPER`, next to this executable (a build tree),
/// or the packaged location. `None` if there's no helper.
pub fn helper_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FLUX_HELPER") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let candidates = [
        exe.parent().map(|d| d.join("flux-helper")),
        exe.parent()
            .and_then(|d| d.parent())
            .map(|d| d.join("libexec/flux-backup/flux-helper")),
        Some(PathBuf::from("/usr/libexec/flux-backup/flux-helper")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// The command that starts the helper as root: `pkexec <helper>`. `$FLUX_PKEXEC` replaces
/// `pkexec` (an empty value runs the helper directly, for tests).
pub fn launcher(helper: PathBuf) -> Vec<OsString> {
    match std::env::var_os("FLUX_PKEXEC") {
        Some(p) if p.is_empty() => vec![helper.into()],
        Some(p) => vec![p, helper.into()],
        None => vec!["pkexec".into(), helper.into()],
    }
}

/// A running, authorized helper.
pub struct HelperClient {
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
}

impl HelperClient {
    /// Start the helper with `argv` and wait until it's ready, which includes the
    /// administrator authenticating. `on_notice` gets job and restore notices;
    /// `on_exit` runs once when the helper goes away.
    pub fn start(
        argv: &[OsString],
        on_notice: Sink,
        on_exit: Box<dyn FnOnce() + Send>,
    ) -> Result<Self> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| ApiError::new("error", "no helper command"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                ApiError::new(
                    "helper_missing",
                    format!("cannot start {}: {e}", program.to_string_lossy()),
                )
            })?;
        let stdin = child.stdin.take().expect("piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped"));
        let mut stderr = child.stderr.take().expect("piped");

        let mut first = String::new();
        let ready = stdout
            .read_line(&mut first)
            .ok()
            .filter(|&n| n > 0)
            .and_then(|_| match serde_json::from_str::<Output>(&first) {
                Ok(Output::Ready { protocol, .. }) => Some(protocol),
                _ => None,
            });
        let Some(protocol) = ready else {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let status = child.wait().ok().and_then(|s| s.code());
            return Err(match status {
                // pkexec: 126 = the dialog was dismissed, 127 = not authorized.
                Some(126) => ApiError::new("auth_cancelled", "Authentication was cancelled."),
                Some(127) => {
                    ApiError::new("auth_failed", "Not authorized to manage system backups.")
                }
                _ => ApiError::new(
                    "helper_failed",
                    format!(
                        "the system helper didn't start{}",
                        match text.trim() {
                            "" => String::new(),
                            t => format!(": {t}"),
                        }
                    ),
                ),
            });
        };
        if protocol != PROTOCOL_VERSION {
            drop(stdin); // it exits on EOF
            let _ = child.wait();
            return Err(ApiError::new(
                "helper_failed",
                format!(
                    "the installed system helper speaks protocol {protocol}, this app needs {PROTOCOL_VERSION}; reinstall Flux Backup"
                ),
            ));
        }

        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (pending, alive) = (pending.clone(), alive.clone());
            std::thread::spawn(move || {
                // Drain stderr so the helper never blocks on it; keep it for the log.
                let stderr_thread = std::thread::spawn(move || {
                    let mut text = String::new();
                    let _ = stderr.read_to_string(&mut text);
                    text
                });
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdout.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    match serde_json::from_str::<Output>(&line) {
                        Ok(Output::Reply { id, ok, err }) => {
                            if let Some(tx) = pending.locked().remove(&id) {
                                let _ = tx.send(match err {
                                    Some(e) => Err(e),
                                    None => Ok(ok.unwrap_or(Value::Null)),
                                });
                            }
                        }
                        Ok(Output::Notice { notice }) => on_notice(notice),
                        Ok(Output::Ready { .. }) => {}
                        Err(e) => tracing::warn!("unreadable line from the helper: {e}"),
                    }
                }
                alive.store(false, Relaxed);
                for (_, tx) in pending.locked().drain() {
                    let _ = tx.send(Err(gone()));
                }
                let _ = child.wait();
                if let Ok(text) = stderr_thread.join()
                    && !text.trim().is_empty()
                {
                    tracing::info!("flux-helper: {}", text.trim());
                }
                on_exit();
            });
        }
        Ok(Self {
            stdin: Mutex::new(Some(stdin)),
            pending,
            next: AtomicU64::new(1),
            alive,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Relaxed)
    }

    /// Send `call` and wait for its reply.
    pub fn call(&self, call: Call) -> Result<Value> {
        if !self.is_alive() {
            return Err(gone());
        }
        let id = self.next.fetch_add(1, Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.locked().insert(id, tx);
        let mut line = serde_json::to_string(&Request { id, call })?;
        line.push('\n');
        let sent = match self.stdin.locked().as_mut() {
            Some(w) => w
                .write_all(line.as_bytes())
                .and_then(|()| w.flush())
                .is_ok(),
            None => false,
        };
        if !sent {
            self.pending.locked().remove(&id);
            return Err(gone());
        }
        rx.recv().unwrap_or_else(|_| Err(gone()))
    }

    /// Ask the helper to exit (it finishes what it's doing first).
    pub fn close(&self) {
        self.stdin.locked().take();
    }
}

impl Drop for HelperClient {
    fn drop(&mut self) {
        self.close();
    }
}

fn gone() -> ApiError {
    ApiError::new(
        "locked",
        "System backups are locked again. Authenticate to continue.",
    )
}
