//! Desktop integration: notifications, suspend inhibition and process priority.
//! Everything here is best-effort. A headless server without D-Bus still backs up fine.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;

use flux_core::profile::{IoPriority, Options};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Normal,
    Critical,
}

/// Show a desktop notification. Failures are logged at debug level and ignored.
pub fn notify(summary: &str, body: &str, urgency: Urgency) {
    // A test on a private bus never shows anything on the desktop's.
    #[cfg(test)]
    if TEST_BUS.get().is_some() {
        if !elsewhere_in_tests() {
            PLAIN.fetch_add(1, Ordering::Relaxed);
        }
        return;
    }
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

/// A button on a notification. What a click does is up to the end of the run
/// ([`follow_up`]), which waits for one a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show the backup in the app (also what clicking the notification itself does).
    Open,
    /// Run the backup again.
    Retry,
}

impl Action {
    fn key(self) -> &'static str {
        match self {
            Action::Open => "open",
            Action::Retry => "retry",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Action::Open => "Open",
            Action::Retry => "Try again",
        }
    }
}

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// The notifications this run showed with buttons, and the connection that hears of a
/// click (the server tells the connection that showed them).
struct Shown {
    conn: zbus::Connection,
    signals: zbus::MessageStream,
    /// Notification id, and the backup it's about.
    ids: Vec<(u32, String)>,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
/// Set by a process that waits for clicks at its end ([`follow_up`]); without it a button
/// would be shown that nothing answers.
static ACTIONS: AtomicBool = AtomicBool::new(false);

/// Plain notifications a test would have shown.
#[cfg(test)]
static PLAIN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The bus to use instead of the desktop's (tests: a private one), and the thread of the
/// test that uses it.
#[cfg(test)]
static TEST_BUS: std::sync::OnceLock<(String, std::thread::ThreadId)> = std::sync::OnceLock::new();

/// A notification from another test while one runs on the private bus: dropped. Tests run
/// in parallel in one process, and a backup's notification would take an answer (a
/// click) meant for that test, which then waited ten minutes for its own.
#[cfg(test)]
fn elsewhere_in_tests() -> bool {
    TEST_BUS
        .get()
        .is_some_and(|(_, t)| *t != std::thread::current().id())
}

async fn session_bus() -> zbus::Result<zbus::Connection> {
    #[cfg(test)]
    if let Some((address, _)) = TEST_BUS.get() {
        return zbus::conn::Builder::address(address.as_str()).build().await;
    }
    zbus::Connection::session().await
}

/// This process waits for clicks at its end ([`follow_up`]): notifications may have
/// buttons.
pub fn enable_actions() {
    ACTIONS.store(true, Ordering::Relaxed);
}

/// [`notify`], with buttons (`actions`) and a click on it opening backup `profile`, when
/// this process answers them ([`enable_actions`]) and the desktop shows buttons;
/// otherwise as [`notify`].
pub fn notify_with(summary: &str, body: &str, urgency: Urgency, profile: &str, actions: &[Action]) {
    #[cfg(test)]
    if elsewhere_in_tests() {
        return;
    }
    if !(ACTIONS.load(Ordering::Relaxed)
        && shown_with_buttons(summary, body, urgency, profile, actions))
    {
        notify(summary, body, urgency);
    }
}

fn shown_with_buttons(
    summary: &str,
    body: &str,
    urgency: Urgency,
    profile: &str,
    actions: &[Action],
) -> bool {
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    zbus::block_on(async {
        if shown.is_none() {
            let Ok(conn) = session_bus().await else {
                return false;
            };
            // Only a server that shows buttons.
            let caps = conn
                .call_method(
                    Some(NOTIFICATIONS),
                    NOTIFICATIONS_PATH,
                    Some(NOTIFICATIONS),
                    "GetCapabilities",
                    &(),
                )
                .await
                .ok()
                .and_then(|m| m.body().deserialize::<Vec<String>>().ok())
                .unwrap_or_default();
            if !caps.iter().any(|c| c == "actions") {
                return false;
            }
            // Listening before the first is shown, so no click is missed.
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(NOTIFICATIONS)
                .build()
            else {
                return false;
            };
            let Ok(signals) = zbus::MessageStream::for_match_rule(rule, &conn, None).await else {
                return false;
            };
            *shown = Some(Shown {
                conn,
                signals,
                ids: Vec::new(),
            });
        }
        let Some(s) = shown.as_mut() else {
            return false;
        };
        let mut keys = vec!["default", "Open"];
        for a in actions {
            keys.extend([a.key(), a.label()]);
        }
        let mut hints: HashMap<&str, zbus::Variant<'_>> = HashMap::new();
        hints.insert(
            "urgency",
            zbus::Variant::from(match urgency {
                Urgency::Normal => 1u8,
                Urgency::Critical => 2u8,
            }),
        );
        hints.insert("desktop-entry", zbus::Variant::from("flux-backup"));
        let icon = match urgency {
            Urgency::Normal => "drive-harddisk",
            Urgency::Critical => "dialog-error",
        };
        let id = s
            .conn
            .call_method(
                Some(NOTIFICATIONS),
                NOTIFICATIONS_PATH,
                Some(NOTIFICATIONS),
                "Notify",
                &("Flux Backup", 0u32, icon, summary, body, keys, hints, -1i32),
            )
            .await
            .ok()
            .and_then(|m| m.body().deserialize::<u32>().ok());
        match id {
            Some(id) => {
                s.ids.push((id, profile.to_owned()));
                true
            }
            None => false,
        }
    })
}

/// How long the end of a run waits for a click: ten minutes, or `FLUX_NOTIFY_WAIT`
/// seconds.
fn click_wait() -> Duration {
    std::env::var("FLUX_NOTIFY_WAIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(Duration::from_secs(600), Duration::from_secs)
}

/// At the end of a run: if it showed notifications with buttons, wait (up to
/// [`click_wait`], or until they're all closed) for one to be clicked, and say which, for
/// which backup.
pub fn follow_up() -> Option<(Action, String)> {
    let shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    if shown.ids.is_empty() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Shown {
            conn: _conn,
            mut signals,
            mut ids,
        } = shown;
        zbus::block_on(async move {
            while let Some(Ok(msg)) = signals.next().await {
                let header = msg.header();
                match header.member().map(|m| m.as_str()) {
                    Some("ActionInvoked") => {
                        if let Ok((id, key)) = msg.body().deserialize::<(u32, String)>()
                            && let Some((_, profile)) = ids.iter().find(|(i, _)| *i == id)
                        {
                            let action = if key == "retry" {
                                Action::Retry
                            } else {
                                Action::Open
                            };
                            let _ = tx.send(Some((action, profile.clone())));
                            return;
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((id, _why)) = msg.body().deserialize::<(u32, u32)>() {
                            ids.retain(|(i, _)| *i != id);
                            if ids.is_empty() {
                                let _ = tx.send(None);
                                return;
                            }
                        }
                    }
                    _ => {}
                }
            }
            let _ = tx.send(None);
        });
    });
    rx.recv_timeout(click_wait()).ok().flatten()
}

/// The app's name on the session bus, and its request to show something.
const APP: &str = "io.github.neuralpunk.FluxBackup";
const APP_PATH: &str = "/io/github/neuralpunk/FluxBackup";
const APP_INTERFACE: &str = "io.github.neuralpunk.FluxBackup.App";

/// The app's program: next to this one (installed together), or on the PATH.
fn app_program() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|me| Some(me.parent()?.join("flux-backup")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "flux-backup".into())
}

/// Show backup `profile` in the app: handed to the app if it's running, else the app is
/// started on it, as a service of its own so it outlives this run (which may be a
/// scheduled backup's, ended when this process exits).
pub fn open_app(profile: &str) {
    let handed = zbus::block_on(async {
        let conn = session_bus().await.ok()?;
        conn.call_method(
            Some(APP),
            APP_PATH,
            Some(APP_INTERFACE),
            "Open",
            &("backup", profile),
        )
        .await
        .ok()
    });
    if handed.is_some() {
        return;
    }
    let app = app_program();
    let mut run = std::process::Command::new("systemd-run");
    run.args(["--user", "--collect", "--quiet"]);
    // The window needs the session's display, which a service may not have been given.
    for var in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if std::env::var_os(var).is_some() {
            run.arg(format!("--setenv={var}"));
        }
    }
    run.arg("--").arg(&app).args(["--show-backup", profile]);
    let started = run
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !started {
        use std::os::unix::process::CommandExt;
        if let Err(e) = std::process::Command::new(&app)
            .args(["--show-backup", profile])
            .process_group(0)
            .spawn()
        {
            tracing::debug!("cannot start {}: {e}", app.display());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    /// A private session bus for the test (never the desktop's own), stopped when dropped.
    struct Bus(std::process::Child);
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A notification server that shows buttons and answers each notification with
    /// `then`: a click on a button (`Some(key)`), or the notification closed (`None`).
    fn server(then: Vec<Option<&'static str>>) {
        let conn = zbus::block_on(async {
            let c = session_bus().await.unwrap();
            c.request_name(NOTIFICATIONS).await.unwrap();
            c
        });
        std::thread::spawn(move || {
            zbus::block_on(async move {
                let mut incoming = zbus::MessageStream::from(&conn);
                let mut next = 0u32;
                let mut then = then.into_iter();
                while let Some(Ok(msg)) = incoming.next().await {
                    let header = msg.header();
                    if header.message_type() != zbus::message::Type::MethodCall {
                        continue;
                    }
                    match header.member().map(|m| m.as_str()) {
                        Some("GetCapabilities") => {
                            let _ = conn.reply(&header, &vec!["actions", "body"]).await;
                        }
                        Some("Notify") => {
                            next += 1;
                            let _ = conn.reply(&header, &next).await;
                            let (member, body) = match then.next().flatten() {
                                Some(key) => ("ActionInvoked", (next, key.to_owned())),
                                None => ("NotificationClosed", (next, String::new())),
                            };
                            if member == "ActionInvoked" {
                                let _ = conn
                                    .emit_signal(
                                        None::<&str>,
                                        NOTIFICATIONS_PATH,
                                        NOTIFICATIONS,
                                        member,
                                        &body,
                                    )
                                    .await;
                            } else {
                                let _ = conn
                                    .emit_signal(
                                        None::<&str>,
                                        NOTIFICATIONS_PATH,
                                        NOTIFICATIONS,
                                        member,
                                        &(next, 2u32),
                                    )
                                    .await;
                            }
                        }
                        _ => {}
                    }
                }
            });
        });
    }

    #[test]
    fn buttons_on_notifications() {
        let Ok(mut child) = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
        else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let _bus = Bus(child);
        TEST_BUS
            .set((address.trim().to_owned(), std::thread::current().id()))
            .unwrap();
        server(vec![Some("retry"), Some("default"), None]);

        // Nothing waits for a click until a process says it will.
        notify_with(
            "Backup failed",
            "x",
            Urgency::Critical,
            "home",
            &[Action::Retry],
        );
        assert_eq!(PLAIN.load(Ordering::Relaxed), 1, "shown without buttons");
        assert_eq!(follow_up(), None);

        enable_actions();
        notify_with(
            "Backup of “Home” failed",
            "x",
            Urgency::Critical,
            "home",
            &[Action::Retry, Action::Open],
        );
        assert_eq!(follow_up(), Some((Action::Retry, "home".into())));
        // Clicking the notification itself opens the backup.
        notify_with(
            "Unusual changes",
            "x",
            Urgency::Critical,
            "docs",
            &[Action::Open],
        );
        assert_eq!(follow_up(), Some((Action::Open, "docs".into())));
        // Closed without a click: the run ends at once.
        notify_with(
            "Restore test failed",
            "x",
            Urgency::Critical,
            "home",
            &[Action::Open],
        );
        let t = std::time::Instant::now();
        assert_eq!(follow_up(), None);
        assert!(t.elapsed() < Duration::from_secs(5));
        // Nothing shown with buttons: nothing to wait for.
        assert_eq!(follow_up(), None);
    }
}
