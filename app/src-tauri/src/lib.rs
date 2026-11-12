//! Flux Backup desktop app (Tauri 2).

mod autostart;
mod backend;
mod session;
mod tray;

use std::sync::Arc;

use tauri::{Emitter, Manager};

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FLUX_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let paths = match flux_core::Paths::resolve(flux_core::Scope::User) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("flux-backup: {e}");
            std::process::exit(1);
        }
    };
    let started_hidden = std::env::args().any(|a| a == "--hidden");
    let request = open_request();
    // One copy at a time: started again, show the running one's window instead (a login
    // start, `--hidden`, just leaves it be), or hand it a file manager's request.
    let bus = match session::claim(!started_hidden, request.as_ref()) {
        session::Start::Another => {
            // Said, not silent: from a terminal (or `tauri dev`, which then stops its
            // dev server) the app would otherwise seem to start and end for no reason.
            if !started_hidden && request.is_none() {
                eprintln!(
                    "Flux Backup is already running (its window may be hidden in the tray): asked it to show its window. To start a fresh copy, quit that one first (tray icon › Quit, or `pkill flux-backup`)."
                );
            }
            return;
        }
        session::Start::First(bus) => bus,
    };
    autostart::refresh();
    let on_close = paths.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
                && stays_in_tray(&on_close)
            {
                api.prevent_close();
                let _ = window.hide();
                tell_about_tray(&on_close);
            }
        })
        .setup(move |app| {
            let backend = backend::Backend::new(
                app.handle().clone(),
                flux_service::Service::new(paths.clone()),
            );
            *backend.open_request.lock().expect("open request") = request;
            app.manage(Arc::new(backend));
            if let Some(bus) = bus {
                let handle = app.handle().clone();
                let opener = app.handle().clone();
                session::serve(
                    bus,
                    move || {
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || tray::show(&h));
                    },
                    move |r| {
                        if let Some(b) = opener.try_state::<Arc<backend::Backend>>() {
                            *b.open_request.lock().expect("open request") = Some(r);
                        }
                        let _ = opener.emit("flux://open", ());
                    },
                );
            }
            // The desktop's accent colour, followed while the app runs.
            let accent = app.handle().clone();
            session::watch_accent(move |c| {
                let _ = accent.emit("flux://accent", c);
            });
            let tray = tray::install(app.handle());
            if let Err(e) = &tray {
                // No tray on this desktop (e.g. GNOME without AppIndicator): carry on.
                tracing::info!("no system tray: {e}");
            }
            // Started at login (`--hidden`): stay in the tray. Without a tray, show the
            // window anyway, or the app would be running where nobody can reach it.
            let hidden = started_hidden && tray.is_ok() && session::tray_host();
            if !hidden && let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend::profiles,
            backend::overview,
            backend::status,
            backend::snapshots,
            backend::list_dir,
            backend::unlock,
            backend::save_key_file,
            backend::set_password,
            backend::replace_password,
            backend::backup_now,
            backend::cancel,
            backend::restore,
            backend::get_profile,
            backend::new_profile,
            backend::validate_profile,
            backend::save_profile,
            backend::delete_profile,
            backend::suggest_id,
            backend::retention_preview,
            backend::exclude_suggestions,
            backend::folder_sizes,
            backend::drives,
            backend::backintime_profiles,
            backend::test_target,
            backend::update_snapshot,
            backend::delete_snapshot,
            backend::delete_snapshots,
            backend::compare,
            backend::runs,
            backend::run_events,
            backend::prune,
            backend::check,
            backend::recovery_kit,
            backend::tools,
            backend::save_recovery_kit,
            backend::export_profile,
            backend::export_formats,
            backend::about,
            backend::open_link,
            backend::vault_security,
            backend::recovery_code,
            backend::strengthen_key,
            backend::set_unlock_file,
            backend::time_hashing,
            backend::lock_vault,
            backend::add_destination,
            backend::remove_destination,
            backend::restore_test,
            backend::restore_tests,
            backend::dismiss_unusual,
            backend::set_webhooks,
            backend::test_webhook,
            backend::find_backups,
            backend::explain,
            backend::save_template,
            backend::storage,
            backend::vault_format,
            backend::upgrade_vault,
            backend::templates,
            backend::new_from_template,
            backend::delete_template,
            backend::coverage,
            backend::export_snapshot,
            backend::recover,
            backend::readiness,
            backend::set_readiness,
            backend::reveal_password,
            backend::export_borg_key,
            backend::copy_unlock_file,
            backend::save_export,
            backend::import_profile,
            backend::open_snapshot_file,
            backend::preview_file,
            backend::duplicate_profile,
            backend::diagnostics,
            backend::find,
            backend::versions,
            backend::restore_preview,
            backend::estimate,
            backend::preferences,
            backend::save_preferences,
            backend::set_pause,
            backend::set_scheduler,
            backend::shutdown_after,
            backend::cancel_shutdown,
            backend::diff_file,
            backend::set_diff_tool,
            backend::set_close_to_tray,
            backend::set_weekly_note,
            backend::set_quiet_hours,
            backend::tray_available,
            backend::desktop_accent,
            backend::set_text_size,
            backend::calendar_preview,
            backend::open_diff_tool,
            backend::autostart,
            backend::set_autostart,
            backend::system_status,
            backend::system_unlock,
            backend::system_lock,
            backend::browse_in_files,
            backend::take_open_request,
            backend::backup_for_path,
            backend::folder_plan,
            backend::apply_folder_change,
            backend::browsing,
            backend::stop_browsing,
        ])
        .build(tauri::generate_context!())
        .expect("error while running Flux Backup")
        .run(|app, event| {
            // A snapshot open in the file manager is unmounted when the app quits.
            if let tauri::RunEvent::Exit = event
                && let Some(b) = app.try_state::<Arc<backend::Backend>>()
            {
                b.user.stop_browsing();
            }
        });
}

/// A file manager's request: `--previous-versions PATH`, `--restore-deleted FOLDER`,
/// `--leave-out PATH` or `--back-up FOLDER`; or a notification's *Open*: `--show-backup ID`.
fn open_request() -> Option<session::Request> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--show-backup")
        && let Some(id) = args.get(i + 1)
    {
        return Some(("backup".into(), id.clone()));
    }
    [
        ("--previous-versions", "versions"),
        ("--restore-deleted", "deleted"),
        ("--leave-out", "exclude"),
        ("--back-up", "include"),
    ]
    .iter()
    .find_map(|(flag, kind)| {
        let i = args.iter().position(|a| a == flag)?;
        let path = std::path::PathBuf::from(args.get(i + 1)?);
        let path = std::path::absolute(&path).unwrap_or(path);
        Some(((*kind).to_owned(), path.display().to_string()))
    })
}

/// Closing the window leaves the app in the tray: unless Preferences say to quit, or no
/// tray shows its icon (then nothing could bring the window back).
fn stays_in_tray(paths: &flux_core::Paths) -> bool {
    !flux_runner::prefs::Settings::load(paths).quit_on_close
        && tray::installed()
        && session::tray_host()
}

/// The first time the window closes into the tray, say where the app went.
fn tell_about_tray(paths: &flux_core::Paths) {
    let mut s = flux_runner::prefs::Settings::load(paths);
    if s.told_about_tray {
        return;
    }
    flux_runner::desktop::notify(
        "Flux Backup is still running",
        "It's in the system tray, showing backups as they run. Quit from its menu, or choose in Preferences to quit when the window is closed.",
        flux_runner::desktop::Urgency::Normal,
    );
    s.told_about_tray = true;
    if let Err(e) = s.save(paths) {
        tracing::warn!("couldn't remember the tray note: {e}");
    }
}
