//! Desktop integration: notifications, suspend inhibition and process priority.
//! Everything here is best-effort. A headless server without D-Bus still backs up fine.

use std::os::fd::OwnedFd;

use flux_core::profile::{IoPriority, Options};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Normal,
    Critical,
}

/// Show a desktop notification. Failures are logged at debug level and ignored.
pub fn notify(summary: &str, body: &str, urgency: Urgency) {
    let mut n = notify_rust::Notification::new();
    n.appname("Flux Backup")
        .summary(summary)
        .body(body)
        .icon(match urgency {
            Urgency::Normal => "drive-harddisk",
            Urgency::Critical => "dialog-error",
        })
        .urgency(match urgency {
            Urgency::Normal => notify_rust::Urgency::Normal,
            Urgency::Critical => notify_rust::Urgency::Critical,
        });
    if let Err(e) = n.show() {
        tracing::debug!("desktop notification failed: {e}");
    }
}

/// Holds a systemd-logind sleep inhibitor; suspend is allowed again when dropped.
pub struct SleepInhibitor {
    _fd: OwnedFd,
}

/// Ask logind not to suspend while a backup runs. `None` if logind isn't reachable or
/// refuses (the backup proceeds anyway).
pub fn inhibit_sleep(why: &str) -> Option<SleepInhibitor> {
    // zbus 6 has no blocking API: run its async call to completion on this thread.
    zbus::block_on(async {
        let conn = zbus::Connection::system().await.ok()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Inhibit",
                &("sleep", "Flux Backup", why, "block"),
            )
            .await
            .map_err(|e| tracing::debug!("logind inhibit failed: {e}"))
            .ok()?;
        let fd: zbus::OwnedFd = reply.body().deserialize().ok()?;
        Some(SleepInhibitor { _fd: fd.into() })
    })
}

/// Lower this thread's CPU and I/O priority. Threads spawned afterwards (the engine's
/// worker pool) inherit it, so call this before starting the engine.
pub fn lower_priority(opts: &Options) {
    if opts.nice > 0
        && let Err(e) = rustix::process::setpriority_process(None, i32::from(opts.nice))
    {
        tracing::debug!("setpriority failed: {e}");
    }
    let class: &[&str] = match opts.io_priority {
        IoPriority::Normal => return,
        IoPriority::BestEffortLow => &["-c", "2", "-n", "7"],
        IoPriority::Idle => &["-c", "3"],
    };
    // No safe ioprio_set wrapper exists; util-linux's ionice is everywhere.
    let tid = rustix::thread::gettid().as_raw_nonzero().get().to_string();
    match std::process::Command::new("ionice")
        .args(class)
        .args(["-p", &tid])
        .status()
    {
        Ok(s) if s.success() => {}
        other => tracing::debug!("ionice failed: {other:?}"),
    }
}

/// The program that schedules and cancels shutdowns: `shutdown`, or `FLUX_SHUTDOWN` (for
/// tests).
fn shutdown_program() -> String {
    std::env::var("FLUX_SHUTDOWN").unwrap_or_else(|_| "shutdown".into())
}

/// Power the computer off in a minute (`shutdown --poweroff +1`, through logind), leaving
/// time to cancel it. `Err` says why it couldn't be scheduled.
pub fn schedule_poweroff() -> Result<(), String> {
    run_shutdown(&[
        "--poweroff",
        "+1",
        "Flux Backup finished; shutting down in a minute (shutdown -c cancels it).",
    ])
}

/// Cancel a scheduled shutdown (`shutdown -c`).
pub fn cancel_poweroff() -> Result<(), String> {
    run_shutdown(&["-c"])
}

fn run_shutdown(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(shutdown_program())
        .args(args)
        .output()
        .map_err(|e| format!("cannot run shutdown: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() {
            format!("shutdown failed ({})", out.status)
        } else {
            err
        })
    }
}
