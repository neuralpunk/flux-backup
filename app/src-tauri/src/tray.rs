//! System tray: open the window, back up every profile, quit. Closing the window quits
//! the app as usual; scheduled backups never need it running.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::backend::Backend;
use flux_service::views::{backup_now_impl, profiles_impl};

const TRAY_ID: &str = "flux";
/// Grey stack (recoloured by GNOME as a symbolic icon); the top pill lights up while a
/// backup runs.
const IDLE: &[u8] = include_bytes!("../icons/tray/idle.png");
const ACTIVE: &[u8] = include_bytes!("../icons/tray/active.png");

/// Profiles with a backup running, as seen in job notices.
static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Track a profile's backup starting or ending, and show it in the tray icon.
pub fn set_running(app: &AppHandle, profile: &str, running: bool) {
    let busy = {
        let mut r = RUNNING.lock().expect("running set");
        if running {
            r.insert(profile.to_owned());
        } else {
            r.remove(profile);
        }
        !r.is_empty()
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let icon = Image::from_bytes(if busy { ACTIVE } else { IDLE });
        if let Ok(icon) = icon {
            let _ = tray.set_icon(Some(icon));
        }
        let _ = tray.set_tooltip(Some(if busy {
            "Flux Backup: backing up"
        } else {
            "Flux Backup"
        }));
    }
}

fn show(app: &AppHandle) {
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
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &all, &PredefinedMenuItem::separator(app)?, &quit],
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
    Ok(())
}
