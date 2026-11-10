//! `flux-helper`: system (root) backups for the desktop app.
//!
//! The app starts it with `pkexec`, so polkit asks for an administrator's password (once
//! per session, see `io.github.neuralpunk.FluxBackup.helper.policy`). It then serves the typed
//! protocol of `flux_service::protocol` on stdin/stdout for the system profiles in
//! `/etc/flux-backup`, using exactly the code the app uses for your own profiles. There is
//! no other way in: no shell commands, no arbitrary files; just the listed calls.
//!
//! It exits when the app closes the pipe (after finishing what it's doing), or after
//! [`IDLE`] without requests, so an unlocked session doesn't stay unlocked forever.
//!
//! `flux-helper --run-backup <id>` runs one backup of a system profile; the session
//! starts that in the background for "Back up now", so the backup outlives the window.

#![deny(clippy::unwrap_used)]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flux_core::progress::{Progress, silent};
use flux_core::sync::Locked;
use flux_core::{Paths, ProfileId, ProfileStore, Scope};
use flux_service::protocol::{self, MAX_LINE, Output, PROTOCOL_VERSION};
use flux_service::{ApiError, Service, watch};

/// Without requests for this long, the helper exits and the app shows system backups as
/// locked again.
const IDLE: Duration = Duration::from_secs(15 * 60);

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FLUX_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .init();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let paths = match Paths::resolve(Scope::System) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("flux-helper: {e}");
            return ExitCode::FAILURE;
        }
    };
    match args.as_slice() {
        [] => session(paths),
        [flag, id] if flag == "--run-backup" => run_backup(&paths, &id.to_string_lossy()),
        [flag] if flag == "--version" => {
            println!(
                "flux-helper {} (protocol {PROTOCOL_VERSION})",
                flux_core::VERSION
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "flux-helper is started by the Flux Backup app; use `flux --system` in a terminal."
            );
            ExitCode::from(2)
        }
    }
}

/// One backup of a system profile, like `flux --system backup <id>`.
fn run_backup(paths: &Paths, id: &str) -> ExitCode {
    let result = (|| -> Result<(), String> {
        let id = ProfileId::new(id).map_err(|e| e.to_string())?;
        let profile = ProfileStore::new(paths)
            .load(&id)
            .map_err(|e| e.to_string())?;
        let r = flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths,
                id: &id,
                profile: &profile,
                name: None,
                dry_run: false,
                trigger: flux_runner::Trigger::Manual,
                prune: true,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        );
        r.outcome.map(|_| ()).map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flux-helper: backup of {id} failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The `flux` CLI that system timers run. Only a root-owned binary that nobody else can
/// write is accepted when running as root, since timers run it as root.
fn flux_cli_for_units(helper: &Path) -> PathBuf {
    let dir = helper.parent().unwrap_or(Path::new("/"));
    let candidates = [
        dir.join("flux"),
        dir.join("../../bin/flux"),
        PathBuf::from("/usr/bin/flux"),
        PathBuf::from("/usr/local/bin/flux"),
    ];
    let root = flux_core::sys::is_root();
    candidates
        .into_iter()
        .filter_map(|p| p.canonicalize().ok())
        .find(|p| {
            use std::os::unix::fs::MetadataExt;
            p.metadata()
                .is_ok_and(|m| m.is_file() && (!root || (m.uid() == 0 && m.mode() & 0o022 == 0)))
        })
        .unwrap_or_else(|| PathBuf::from("/usr/bin/flux"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

type Out = Arc<Mutex<std::io::Stdout>>;

fn send(out: &Out, msg: &Output) {
    if let Ok(mut line) = serde_json::to_string(msg) {
        line.push('\n');
        let mut o = out.locked();
        // The app went away: nothing to do but finish.
        let _ = o.write_all(line.as_bytes()).and_then(|()| o.flush());
    }
}

fn session(paths: Paths) -> ExitCode {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("/usr/libexec/flux-backup/flux-helper"));
    let service = Arc::new(Service::with_commands(
        paths.clone(),
        vec![exe.clone().into_os_string(), "--run-backup".into()],
        flux_cli_for_units(&exe),
    ));
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    if let Some(uid) = invoker() {
        tracing::info!("system backups unlocked for uid {uid}");
    }
    send(
        &out,
        &Output::Ready {
            version: flux_core::VERSION.to_owned(),
            protocol: PROTOCOL_VERSION,
            uid: flux_core::sys::euid(),
        },
    );

    {
        let out = out.clone();
        watch::start(
            paths,
            Arc::new(move |notice| send(&out, &Output::Notice { notice })),
        );
    }

    let last = Arc::new(AtomicU64::new(now_secs()));
    let busy = Arc::new(AtomicUsize::new(0));
    {
        let (last, busy) = (last.clone(), busy.clone());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(20));
                if busy.load(SeqCst) == 0
                    && now_secs().saturating_sub(last.load(SeqCst)) > IDLE.as_secs()
                {
                    tracing::info!(
                        "idle for {} minutes; locking system backups",
                        IDLE.as_secs() / 60
                    );
                    std::process::exit(0);
                }
            }
        });
    }

    let mut input = BufReader::new(std::io::stdin().lock());
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // Read at most MAX_LINE + 1 bytes of a line; longer lines are refused and skipped.
        let n = match (&mut input)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
        {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
            skip_line(&mut input);
            send(
                &out,
                &reply(None, Err(ApiError::new("bad_request", "request too long"))),
            );
            continue;
        }
        last.store(now_secs(), SeqCst);
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request = match protocol::parse_request(line) {
            Ok(r) => r,
            Err((id, e)) => {
                tracing::warn!("refused a request: {}", e.message);
                send(&out, &reply(id, Err(e)));
                continue;
            }
        };
        tracing::info!("{}", request.call.method());
        busy.fetch_add(1, SeqCst);
        let (service, out, busy, last) = (service.clone(), out.clone(), busy.clone(), last.clone());
        std::thread::spawn(move || {
            let notify = |notice| send(&out, &Output::Notice { notice });
            let result = protocol::dispatch(&service, request.call, &notify);
            send(&out, &reply(Some(request.id), result));
            last.store(now_secs(), SeqCst);
            busy.fetch_sub(1, SeqCst);
        });
    }

    // The app closed the pipe: finish running requests (a restore, say), then exit.
    while busy.load(SeqCst) > 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
    ExitCode::SUCCESS
}

fn skip_line(input: &mut impl BufRead) {
    loop {
        let (done, used) = match input.fill_buf() {
            Ok([]) | Err(_) => return,
            Ok(b) => match b.iter().position(|&c| c == b'\n') {
                Some(i) => (true, i + 1),
                None => (false, b.len()),
            },
        };
        input.consume(used);
        if done {
            return;
        }
    }
}

fn reply(id: Option<u64>, result: flux_service::Result<serde_json::Value>) -> Output {
    let (ok, err) = match result {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    Output::Reply {
        id: id.unwrap_or(0),
        ok,
        err,
    }
}

/// Who asked (`PKEXEC_UID`), for the log.
fn invoker() -> Option<u32> {
    std::env::var("PKEXEC_UID").ok()?.parse().ok()
}
