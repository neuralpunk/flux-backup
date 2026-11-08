//! Flux Backup desktop app (Tauri 2).

mod autostart;
mod backend;
mod tray;

use std::sync::Arc;

use tauri::Manager;

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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let backend = backend::Backend::new(
                app.handle().clone(),
                flux_service::Service::new(paths.clone()),
            );
            app.manage(Arc::new(backend));
            let tray = tray::install(app.handle());
            if let Err(e) = &tray {
                // No tray on this desktop (e.g. GNOME without AppIndicator): carry on.
                tracing::info!("no system tray: {e}");
            }
            // Started at login (`--hidden`): stay in the tray. Without a tray, show the
            // window anyway, or the app would be running where nobody can reach it.
            let hidden = std::env::args().any(|a| a == "--hidden") && tray.is_ok();
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
            backend::drives,
            backend::test_target,
            backend::update_snapshot,
            backend::delete_snapshot,
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
            backend::save_export,
            backend::import_profile,
            backend::open_snapshot_file,
            backend::duplicate_profile,
            backend::diagnostics,
            backend::find,
            backend::versions,
            backend::restore_preview,
            backend::estimate,
            backend::preferences,
            backend::save_preferences,
            backend::set_pause,
            backend::autostart,
            backend::set_autostart,
            backend::system_status,
            backend::system_unlock,
            backend::system_lock,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Flux Backup");
}
