//! Flux Backup desktop app (Tauri 2).

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
            if let Err(e) = tray::install(app.handle()) {
                // No tray on this desktop (e.g. GNOME without AppIndicator): carry on.
                tracing::info!("no system tray: {e}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend::profiles,
            backend::overview,
            backend::snapshots,
            backend::list_dir,
            backend::unlock,
            backend::save_key_file,
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
            backend::system_status,
            backend::system_unlock,
            backend::system_lock,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Flux Backup");
}
