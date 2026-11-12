//! System tray: open the window, back up every profile, pause, quit. Closing the window
//! leaves the app here (unless Preferences say to quit); scheduled backups never need it
//! running either way.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::backend::Backend;
use flux_core::progress::{Phase, ProgressSnapshot, time_left_words};
use flux_service::views::{backup_now_impl, profiles_impl};

const TRAY_ID: &str = "flux";
/// Grey stack (recoloured by GNOME as a symbolic icon); the top pill lights up while a
/// backup runs.
const IDLE: &[u8] = include_bytes!("../icons/tray/idle.png");
const ACTIVE: &[u8] = include_bytes!("../icons/tray/active.png");

/// The tray icon was made (the desktop may still not show it: see `session::tray_host`).
static INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Profiles with a backup running, as seen in job notices, each with how far it is
/// ("21% · about 6 minutes left") once that's known.
static RUNNING: Mutex<BTreeMap<String, Option<String>>> = Mutex::new(BTreeMap::new());
/// The tooltip last set, so it's set again only when the words change.
static TOOLTIP: Mutex<String> = Mutex::new(String::new());

/// How far a backup is, in words: "21% · about 6 minutes left" while copying.
fn how_far(p: &ProgressSnapshot) -> Option<String> {
    if p.phase != Phase::Transferring || p.bytes_total == 0 {
        return None;
    }
    let pct = (p.bytes_done as f64 / p.bytes_total as f64 * 100.0).min(100.0) as u64;
    Some(match p.eta_secs {
        Some(eta) => format!("{pct}% · {}", time_left_words(eta)),
        None => format!("{pct}%"),
    })
}

/// The tray's tooltip for what's running.
fn tooltip(running: &BTreeMap<String, Option<String>>) -> String {
    match running.len() {
        0 => "Flux Backup".into(),
        1 => match running.values().next().cloned().flatten() {
            Some(far) => format!("Flux Backup: backing up · {far}"),
            None => "Flux Backup: backing up".into(),
        },
        n => format!("Flux Backup: {n} backups running"),
    }
}

/// Track a profile's backup starting, going on (`progress`) or ending, and show it in the
/// tray icon and its tooltip.
pub fn set_running(
    app: &AppHandle,
    profile: &str,
    running: bool,
    progress: Option<&ProgressSnapshot>,
) {
    let (was, busy, tip) = {
        let mut r = RUNNING.lock().expect("running set");
        let was = !r.is_empty();
        if running {
            let far = progress.and_then(how_far);
            let entry = r.entry(profile.to_owned()).or_default();
            if far.is_some() || progress.is_some_and(|p| p.phase != Phase::Transferring) {
                *entry = far;
            }
        } else {
            r.remove(profile);
        }
        (was, !r.is_empty(), tooltip(&r))
    };
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    // The icon only when it changes: this is called with every progress message of a
    // running backup (five a second), and setting the icon writes a new image file and has
    // the panel load it again each time.
    if was != busy
        && let Ok(icon) = Image::from_bytes(if busy { ACTIVE } else { IDLE })
    {
        let _ = tray.set_icon(Some(icon));
    }
    // The words change every few seconds at most (rounded to the percent and minute).
    let mut last = TOOLTIP.lock().expect("tooltip");
    if *last != tip {
        let _ = tray.set_tooltip(Some(&tip));
        *last = tip;
    }
}

/// Bring the window back (from the tray, or from behind other windows).
pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Flux Backup", true, None::<&str>)?;
    let all = MenuItem::with_id(
        app,
        "backup-all",
        "Back up all profiles now",
        true,
        None::<&str>,
    )?;
    let pause_hour = MenuItem::with_id(
        app,
        "pause-hour",
        "Pause scheduled backups for an hour",
        true,
        None::<&str>,
    )?;
    let pause_tomorrow = MenuItem::with_id(
        app,
        "pause-tomorrow",
        "Pause scheduled backups until tomorrow",
        true,
        None::<&str>,
    )?;
    let resume = MenuItem::with_id(
        app,
        "pause-off",
        "Resume scheduled backups",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &all,
            &PredefinedMenuItem::separator(app)?,
            &pause_hour,
            &pause_tomorrow,
            &resume,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Flux Backup")
        .icon(Image::from_bytes(IDLE)?)
        .menu(&menu)
        .show_menu_on_left_click(false);
    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "open" => show(app),
        "backup-all" => {
            let state = app.state::<Arc<Backend>>().inner().user.clone();
            std::thread::spawn(move || {
                if let Ok(list) = profiles_impl(&state) {
                    for p in list.iter().filter(|p| p.errors.is_empty() && !p.running) {
                        // Vaults whose password isn't available are skipped (their error says why).
                        if let Err(e) = backup_now_impl(&state, p.id.clone()) {
                            tracing::info!("not backing up {}: {}", p.id, e.code());
                        }
                    }
                }
            });
        }
        id @ ("pause-hour" | "pause-tomorrow" | "pause-off") => {
            let state = app.state::<Arc<Backend>>().inner().user.clone();
            let which = id.trim_start_matches("pause-").to_owned();
            match flux_service::prefs::set_pause_impl(&state, which) {
                // The window shows the pause; tell it.
                Ok(_) => {
                    let _ = app.emit("flux://pause", ());
                }
                Err(e) => tracing::warn!("pause: {}", e.message),
            }
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show(tray.app_handle());
        }
    })
    .build(app)?;
    INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(phase: Phase, done: u64, total: u64, eta: Option<f64>) -> ProgressSnapshot {
        ProgressSnapshot {
            phase,
            scanned: 0,
            entries_total: 0,
            entries_done: 0,
            bytes_total: total,
            bytes_done: done,
            warnings: 0,
            elapsed_secs: 0.0,
            bytes_per_sec: 0.0,
            eta_secs: eta,
        }
    }

    #[test]
    fn the_tooltip_says_how_far_and_how_long() {
        let far = how_far(&snap(Phase::Transferring, 21, 100, Some(359.0)));
        assert_eq!(far.as_deref(), Some("21% · about 6 minutes left"));
        assert_eq!(how_far(&snap(Phase::Scanning, 0, 0, None)), None);
        let mut r = BTreeMap::new();
        assert_eq!(tooltip(&r), "Flux Backup");
        r.insert("home".to_owned(), None);
        assert_eq!(tooltip(&r), "Flux Backup: backing up");
        r.insert("home".to_owned(), far);
        assert_eq!(
            tooltip(&r),
            "Flux Backup: backing up · 21% · about 6 minutes left"
        );
        r.insert("docs".to_owned(), None);
        assert_eq!(tooltip(&r), "Flux Backup: 2 backups running");
    }
}
