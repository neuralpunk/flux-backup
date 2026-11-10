//! Tauri commands. Your own profiles are served in-process by `flux-service`; system
//! profiles (shown with ids prefixed `system:`) go to `flux-helper`, which runs as root
//! after an administrator authenticated through polkit. Both speak the same [`Call`]s, so
//! every command is one route.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use flux_runner::jobs::Message;
use flux_service::client::{self, HelperClient};
use flux_service::protocol::{Call, dispatch};
use flux_service::watch::{self, Notice};
use flux_service::{ApiError, Result, Service};

/// Prefix of system profile ids in the frontend.
pub const SYSTEM: &str = "system:";

pub struct Backend {
    pub user: Arc<Service>,
    helper: Mutex<Option<Arc<HelperClient>>>,
    app: AppHandle,
}

/// Send a notice to the window as the events the frontend listens for.
fn emit(app: &AppHandle, notice: Notice) {
    let _ = match notice {
        Notice::Job { profile, message } => {
            match &message {
                Message::Hello { .. } | Message::Progress { .. } => {
                    crate::tray::set_running(app, &profile, true)
                }
                Message::Finished { .. } => crate::tray::set_running(app, &profile, false),
                Message::Event { .. } => {}
            }
            app.emit(
                "flux://job",
                json!({ "profile": profile, "message": message }),
            )
        }
        Notice::Changed { profile } => app.emit("flux://changed", json!({ "profile": profile })),
        Notice::Restore { profile, progress } => app.emit(
            "flux://restore",
            json!({ "profile": profile, "progress": progress }),
        ),
    };
}

fn qualify_id(v: &mut Value) {
    if let Some(id) = v.as_str()
        && !id.is_empty()
        && !id.starts_with(SYSTEM)
    {
        *v = Value::String(format!("{SYSTEM}{id}"));
    }
}

/// Rewrite the ids in a helper reply to their `system:` form.
fn qualify(method: &str, v: &mut Value) {
    match method {
        "profiles" => {
            if let Some(list) = v.as_array_mut() {
                for p in list {
                    qualify_id(&mut p["id"]);
                }
            }
        }
        "overview" => qualify_id(&mut v["profile"]["id"]),
        "duplicate_profile" => qualify_id(v),
        "get_profile" | "new_profile" | "save_profile" | "import_profile"
            if v.get("id").is_some() =>
        {
            qualify_id(&mut v["id"]);
        }
        _ => {}
    }
}

#[derive(Serialize)]
pub struct SystemStatus {
    /// A helper is installed (system backups can be managed here).
    available: bool,
    unlocked: bool,
}

impl Backend {
    pub fn new(app: AppHandle, user: Service) -> Self {
        let user = Arc::new(user);
        {
            let app = app.clone();
            watch::start(user.paths.clone(), Arc::new(move |n| emit(&app, n)));
        }
        // Vaults whose "lock again after" time has passed without use are locked again.
        // (System backups lock as a whole when their helper stops after 15 idle minutes.)
        {
            let (app, user) = (app.clone(), user.clone());
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    for id in user.expire_idle() {
                        let _ = app.emit("flux://locked", json!({ "profile": id }));
                    }
                }
            });
        }
        Self {
            user,
            helper: Mutex::new(None),
            app,
        }
    }

    fn helper(&self) -> Option<Arc<HelperClient>> {
        self.helper
            .lock()
            .expect("helper")
            .clone()
            .filter(|h| h.is_alive())
    }

    pub fn system_status(&self) -> SystemStatus {
        SystemStatus {
            available: client::helper_path().is_some(),
            unlocked: self.helper().is_some(),
        }
    }

    /// Start the helper; this is where the polkit dialog appears.
    pub fn unlock_system(&self) -> Result<()> {
        if self.helper().is_some() {
            return Ok(());
        }
        let path = client::helper_path().ok_or_else(|| {
            ApiError::new(
                "helper_missing",
                "The system helper isn't installed (flux-helper). Install Flux Backup from a package to manage system backups.",
            )
        })?;
        let notices = {
            let app = self.app.clone();
            Arc::new(move |mut n: Notice| {
                let p = n.profile_mut();
                *p = format!("{SYSTEM}{p}");
                emit(&app, n);
            })
        };
        let on_exit = {
            let app = self.app.clone();
            Box::new(move || {
                if let Some(b) = app.try_state::<Arc<Backend>>() {
                    b.helper.lock().expect("helper").take();
                }
                let _ = app.emit("flux://system", json!({ "unlocked": false }));
            })
        };
        let h = HelperClient::start(&client::launcher(path), notices, on_exit)?;
        *self.helper.lock().expect("helper") = Some(Arc::new(h));
        let _ = self.app.emit("flux://system", json!({ "unlocked": true }));
        Ok(())
    }

    pub fn lock_system(&self) {
        if let Some(h) = self.helper.lock().expect("helper").take() {
            h.close();
        }
    }

    /// Run `call` for your profiles, or through the helper for system ones (`system`, or
    /// a `system:` id).
    pub fn route(&self, mut call: Call, mut system: bool) -> Result<Value> {
        if let Some(id) = call.profile_id_mut()
            && let Some(rest) = id.strip_prefix(SYSTEM)
        {
            *id = rest.to_owned();
            system = true;
        }
        if matches!(call, Call::Profiles) {
            return self.all_profiles();
        }
        if !system {
            let app = self.app.clone();
            return dispatch(&self.user, call, &move |n| emit(&app, n));
        }
        let helper = self
            .helper()
            .ok_or_else(|| ApiError::new("locked", "Authenticate to manage system backups."))?;
        let method = call.method();
        let mut v = helper.call(call)?;
        qualify(method, &mut v);
        Ok(v)
    }

    /// Your profiles, then the system's if unlocked.
    fn all_profiles(&self) -> Result<Value> {
        let mut list = serde_json::to_value(flux_service::views::profiles_impl(&self.user)?)?;
        if let Some(h) = self.helper() {
            match h.call(Call::Profiles) {
                Ok(mut sys) => {
                    qualify("profiles", &mut sys);
                    if let (Some(l), Some(s)) = (list.as_array_mut(), sys.as_array_mut()) {
                        l.append(s);
                    }
                }
                Err(e) => tracing::warn!("system profiles: {e}"),
            }
        }
        Ok(list)
    }
}

type St<'a> = tauri::State<'a, Arc<Backend>>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::new("error", e.to_string()))?
}

async fn route(state: St<'_>, call: Call, system: bool) -> Result<Value> {
    let b = state.inner().clone();
    blocking(move || b.route(call, system)).await
}

macro_rules! commands {
    ($( $name:ident ( $($arg:ident : $ty:ty),* ) => $call:expr ;)*) => {$(
        #[tauri::command]
        // One parameter per field the page sends: the IPC call's shape.
        #[allow(clippy::too_many_arguments)]
        pub async fn $name(state: St<'_>, $($arg: $ty),*) -> Result<Value> {
            route(state, $call, false).await
        }
    )*};
}

commands! {
    profiles() => Call::Profiles;
    overview(id: String) => Call::Overview { id };
    status(id: String) => Call::Status { id };
    snapshots(id: String) => Call::Snapshots { id };
    list_dir(id: String, snapshot: String, path: String) => Call::ListDir { id, snapshot, path };
    unlock(id: String, password: String, create: Option<bool>)
        => Call::Unlock { id, password, create: create.unwrap_or(false) };
    save_key_file(id: String) => Call::SaveKeyFile { id };
    set_password(id: String, password: String) => Call::SetPassword { id, password };
    backup_now(id: String, shutdown: Option<bool>) => Call::BackupNow { id, shutdown: shutdown.unwrap_or(false) };
    shutdown_after(id: String, on: bool) => Call::ShutdownAfter { id, on };
    cancel_shutdown() => Call::CancelShutdown;
    diff_file(id: String, path: String, old: String, new: Option<String>) => Call::DiffFile { id, path, old, new };
    set_diff_tool(tool: Option<String>) => Call::SetDiffTool { tool };
    cancel(id: String) => Call::Cancel { id };
    restore(id: String, snapshot: String, paths: Vec<String>, to: Option<String>, overwrite: bool, exact: Option<bool>)
        => Call::Restore { id, snapshot, paths, to, overwrite, exact: exact.unwrap_or(false) };
    get_profile(id: String) => Call::GetProfile { id };
    save_profile(id: String, profile: Value, create: bool) => Call::SaveProfile { id, profile, create };
    delete_profile(id: String) => Call::DeleteProfile { id };
    retention_preview(id: String, retention: Value) => Call::RetentionPreview { id, retention };
    exclude_suggestions(sources: Vec<String>) => Call::ExcludeSuggestions { sources };
    drives() => Call::Drives;
    backintime_profiles() => Call::BackInTime;
    update_snapshot(id: String, snapshot: String, name: Option<String>, set_name: bool, pinned: Option<bool>,
        note: Option<String>, set_note: Option<bool>, tags: Option<Vec<String>>)
        => Call::UpdateSnapshot { id, snapshot, name, set_name, pinned, note, set_note: set_note.unwrap_or(false), tags };
    delete_snapshot(id: String, snapshot: String) => Call::DeleteSnapshot { id, snapshot };
    compare(id: String, old: String, new: Option<String>) => Call::Compare { id, old, new };
    runs(id: String, limit: usize) => Call::Runs { id, limit };
    run_events(id: String, run_id: String) => Call::RunEvents { id, run_id };
    prune(id: String, dry_run: bool) => Call::Prune { id, dry_run };
    check(id: String, read_data: bool) => Call::Check { id, read_data };
    recovery_kit(id: String) => Call::RecoveryKit { id };
    export_profile(id: String, format: String) => Call::ExportProfile { id, format };
    duplicate_profile(id: String) => Call::DuplicateProfile { id };
    diagnostics(id: String) => Call::Diagnostics { id };
    find(id: String, query: String) => Call::Find { id, query };
    preferences() => Call::Preferences;
    save_preferences(notify_success: bool, default_exclude: Option<Vec<String>>, one_at_a_time: Option<bool>)
        => Call::SavePreferences { notify_success, default_exclude, one_at_a_time };
    set_pause(pause: String) => Call::SetPause { pause };
    set_scheduler(scheduler: String) => Call::SetScheduler { scheduler };
    versions(id: String, path: String) => Call::Versions { id, path };
    restore_preview(id: String, snapshot: String, paths: Vec<String>, to: Option<String>, overwrite: bool, exact: Option<bool>)
        => Call::RestorePreview { id, snapshot, paths, to, overwrite, exact: exact.unwrap_or(false) };
    vault_security(id: String) => Call::VaultSecurity { id };
    strengthen_key(id: String, hashing: flux_core::profile::PasswordHashing) => Call::StrengthenKey { id, hashing };
    set_unlock_file(id: String, path: Option<String>, create: Option<bool>)
        => Call::SetUnlockFile { id, path, create: create.unwrap_or(false) };
    time_hashing(hashing: flux_core::profile::PasswordHashing) => Call::TimeHashing { hashing };
    lock_vault(id: String) => Call::Lock { id };
    add_destination(id: String, destination: Value, password: Option<String>)
        => Call::AddDestination { id, destination, password };
    remove_destination(id: String, name: String) => Call::RemoveDestination { id, name };
    restore_test(id: String) => Call::RestoreTest { id };
    restore_tests(id: String) => Call::RestoreTests { id };
    dismiss_unusual(id: String, snapshot: String) => Call::DismissUnusual { id, snapshot };
    set_webhooks(webhooks: Value) => Call::SetWebhooks { webhooks };
    test_webhook(webhook: Value) => Call::TestWebhook { webhook };
    find_backups(target: Value, password: Option<String>) => Call::FindBackups { target, password };
    explain(profile: Value, path: String) => Call::Explain { profile, path };
    save_template(id: String, name: String) => Call::SaveTemplate { id, name };
    storage(id: String) => Call::Storage { id };
    vault_format(id: String) => Call::VaultFormat { id };
    upgrade_vault(id: String) => Call::UpgradeVault { id };
    coverage(profile: Value, dir: String) => Call::Coverage { profile, dir };
    export_snapshot(id: String, snapshot: String, paths: Vec<String>, format: flux_runner::archive::Format,
        dir: String, checksums: Option<flux_runner::archive::Checksums>)
        => Call::ExportSnapshot { id, snapshot, paths, format, dir, checksums };
    recover(target: Value, engine: flux_core::Engine, kept: flux_core::profile::KeptAs,
        then: flux_runner::recover::Then, id: Option<String>, password: Option<String>)
        => Call::Recover { target, engine, kept, then, id, password };
    readiness(id: String) => Call::Readiness { id };
    set_readiness(id: String, key: String, yes: bool) => Call::SetReadiness { id, key, yes };
}

/// Open two files in the comparison program chosen in the preferences (the program comes
/// from the settings, never from the page). Started by the app, as the user.
#[tauri::command]
pub async fn open_diff_tool(state: St<'_>, left: String, right: String) -> Result<()> {
    let b = state.inner().clone();
    blocking(move || {
        let tool = b.route(Call::DiffTool, false)?;
        let cmd = tool
            .get(0)
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                ApiError::new(
                    "error",
                    "No comparison program is set up; choose one in Preferences.",
                )
            })?
            .to_owned();
        let mut parts = cmd.split_whitespace();
        let program = parts.next().unwrap_or_default();
        std::process::Command::new(program)
            .args(parts)
            .arg(&left)
            .arg(&right)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ApiError::new("error", format!("couldn't start {program}: {e}")))?;
        Ok(())
    })
    .await
}

/// Copy a file out of a snapshot (read-only, in a private temporary folder) and open it
/// in its usual program. The program is started by the app, as the user.
#[tauri::command]
pub async fn open_snapshot_file(
    state: St<'_>,
    id: String,
    snapshot: String,
    path: String,
) -> Result<String> {
    let b = state.inner().clone();
    blocking(move || {
        let v = b.route(Call::OpenFile { id, snapshot, path }, false)?;
        let local = v.as_str().unwrap_or_default().to_owned();
        std::process::Command::new("xdg-open")
            .arg(&local)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ApiError::new("error", format!("couldn't open {local}: {e}")))?;
        Ok(local)
    })
    .await
}

/// Versions for the About dialog: this app, the `flux` command and the helper it would
/// run, the system, and the WebKitGTK that draws the window.
#[tauri::command]
pub async fn about() -> Result<Value> {
    blocking(|| {
        let mut v = serde_json::to_value(flux_service::extras::about_impl())?;
        v["webkit"] = json!(tauri::webview_version().ok());
        Ok(v)
    })
    .await
}

/// The project's own pages; the page names one, never a URL.
const LINKS: &[(&str, &str)] = &[
    ("website", "https://neuralpunk.github.io/flux-backup/"),
    (
        "guide",
        "https://neuralpunk.github.io/flux-backup/guide.html",
    ),
    ("issues", "https://github.com/neuralpunk/flux-backup/issues"),
    ("source", "https://github.com/neuralpunk/flux-backup"),
    (
        "changes",
        "https://github.com/neuralpunk/flux-backup/blob/main/CHANGELOG.md",
    ),
];

/// Open one of the project's pages in the web browser.
#[tauri::command]
pub async fn open_link(which: String) -> Result<()> {
    let url = LINKS
        .iter()
        .find(|(k, _)| *k == which)
        .map(|(_, u)| *u)
        .ok_or_else(|| ApiError::new("error", format!("unknown link {which}")))?;
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| ApiError::new("error", format!("couldn't open {url}: {e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn export_formats() -> Result<Value> {
    Ok(serde_json::to_value(
        flux_service::edit::export_formats_impl(),
    )?)
}

/// Save a profile's export into `dir`, a folder the user picked (written by the app, as
/// the user, also for system profiles). Returns the file written.
#[tauri::command]
pub async fn save_export(state: St<'_>, id: String, format: String, dir: String) -> Result<String> {
    let b = state.inner().clone();
    blocking(move || {
        let e = b.route(Call::ExportProfile { id, format }, false)?;
        let name = e["filename"].as_str().unwrap_or("profile-export.txt");
        // A plain file name from the export, nothing else.
        let name = std::path::Path::new(name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "profile-export.txt".into());
        let path = std::path::PathBuf::from(dir).join(name);
        let mode = if e["executable"].as_bool().unwrap_or(false) {
            0o755
        } else {
            0o644
        };
        flux_core::store::write_atomic(
            &path,
            e["contents"].as_str().unwrap_or_default().as_bytes(),
            mode,
        )
        .map_err(|err| ApiError::new("error", format!("{}: {err}", path.display())))?;
        Ok(path.display().to_string())
    })
    .await
}

/// Import the Flux profile in file `path` (read by the app, as the user).
#[tauri::command]
pub async fn import_profile(state: St<'_>, path: String, system: Option<bool>) -> Result<Value> {
    let b = state.inner().clone();
    blocking(move || {
        let meta =
            std::fs::metadata(&path).map_err(|e| ApiError::new("error", format!("{path}: {e}")))?;
        if meta.len() > 1 << 20 {
            return Err(ApiError::new(
                "error",
                "That file is too large to be a profile.",
            ));
        }
        let toml = std::fs::read_to_string(&path)
            .map_err(|e| ApiError::new("error", format!("{path}: {e}")))?;
        b.route(
            Call::ImportProfile { toml, id: None },
            system.unwrap_or(false),
        )
    })
    .await
}

/// Whether rsync and borg are installed here.
#[tauri::command]
pub async fn tools() -> Result<Value> {
    Ok(serde_json::to_value(flux_service::views::tools_impl())?)
}

/// Save a profile's recovery kit (guide and script) into `dir`, a folder the user picked.
/// The files are written by the app itself, as the user, also for system profiles.
#[tauri::command]
pub async fn save_recovery_kit(state: St<'_>, id: String, dir: String) -> Result<Vec<String>> {
    let b = state.inner().clone();
    blocking(move || {
        let kit = b.route(Call::RecoveryKit { id: id.clone() }, false)?;
        let short = id.strip_prefix(SYSTEM).unwrap_or(&id);
        let dir = std::path::PathBuf::from(dir);
        let mut written = Vec::new();
        for (key, name, mode) in [
            ("recovery", "FLUX-RECOVERY.txt", 0o644),
            ("script", "flux-backup.sh", 0o755),
        ] {
            let text = kit[key].as_str().unwrap_or_default();
            let path = dir.join(format!("{short}-{name}"));
            flux_core::store::write_atomic(&path, text.as_bytes(), mode)
                .map_err(|e| ApiError::new("error", format!("{}: {e}", path.display())))?;
            written.push(path.display().to_string());
        }
        Ok(written)
    })
    .await
}

// Calls that aren't about an existing profile say which scope they're for.

#[tauri::command]
pub async fn new_profile(state: St<'_>, system: Option<bool>) -> Result<Value> {
    route(state, Call::NewProfile, system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn templates(state: St<'_>, system: Option<bool>) -> Result<Value> {
    route(state, Call::Templates, system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn new_from_template(
    state: St<'_>,
    template: String,
    system: Option<bool>,
) -> Result<Value> {
    route(
        state,
        Call::NewFromTemplate { template },
        system.unwrap_or(false),
    )
    .await
}

#[tauri::command]
pub async fn delete_template(
    state: St<'_>,
    template: String,
    system: Option<bool>,
) -> Result<Value> {
    route(
        state,
        Call::DeleteTemplate { template },
        system.unwrap_or(false),
    )
    .await
}

#[tauri::command]
pub async fn validate_profile(
    state: St<'_>,
    profile: Value,
    system: Option<bool>,
) -> Result<Value> {
    route(
        state,
        Call::ValidateProfile { profile },
        system.unwrap_or(false),
    )
    .await
}

/// What a profile being edited would back up. System profiles are counted by the helper,
/// which can read everything they back up.
#[tauri::command]
pub async fn estimate(state: St<'_>, profile: Value, system: Option<bool>) -> Result<Value> {
    route(state, Call::Estimate { profile }, system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn suggest_id(state: St<'_>, name: String, system: Option<bool>) -> Result<Value> {
    route(state, Call::SuggestId { name }, system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn test_target(state: St<'_>, profile: Value, system: Option<bool>) -> Result<Value> {
    route(state, Call::TestTarget { profile }, system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn system_status(state: St<'_>) -> Result<SystemStatus> {
    Ok(state.system_status())
}

#[tauri::command]
pub async fn system_unlock(state: St<'_>) -> Result<()> {
    let b = state.inner().clone();
    blocking(move || b.unlock_system()).await
}

#[tauri::command]
pub async fn system_lock(state: St<'_>) -> Result<()> {
    state.lock_system();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_replies_get_system_ids() {
        let mut list = json!([{ "id": "etc" }, { "id": "system:already" }]);
        qualify("profiles", &mut list);
        assert_eq!(
            list,
            json!([{ "id": "system:etc" }, { "id": "system:already" }])
        );
        let mut o = json!({ "profile": { "id": "etc" } });
        qualify("overview", &mut o);
        assert_eq!(o["profile"]["id"], "system:etc");
        let mut new = json!({ "id": "", "profile": {} });
        qualify("new_profile", &mut new);
        assert_eq!(new["id"], "", "a new profile has no id yet");
        let mut s = json!({ "id": "etc", "saved": true });
        qualify("save_profile", &mut s);
        assert_eq!(s["id"], "system:etc");
        let mut imp = json!({ "id": "etc", "issues": [] });
        qualify("import_profile", &mut imp);
        assert_eq!(imp["id"], "system:etc");
        let mut dup = json!("etc-copy");
        qualify("duplicate_profile", &mut dup);
        assert_eq!(dup, "system:etc-copy");
        let mut other = json!("etc");
        qualify("suggest_id", &mut other);
        assert_eq!(other, "etc", "suggested ids stay plain");
    }
}

/// Whether Flux starts (in the tray) when you log in.
#[tauri::command]
pub fn autostart() -> bool {
    crate::autostart::enabled()
}

#[tauri::command]
pub fn set_autostart(on: bool) -> std::result::Result<(), String> {
    crate::autostart::set(on).map_err(|e| e.to_string())
}
