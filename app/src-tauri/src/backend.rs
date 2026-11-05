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
        "get_profile" | "new_profile" | "save_profile" if v.get("id").is_some() => {
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
        pub async fn $name(state: St<'_>, $($arg: $ty),*) -> Result<Value> {
            route(state, $call, false).await
        }
    )*};
}

commands! {
    profiles() => Call::Profiles;
    overview(id: String) => Call::Overview { id };
    snapshots(id: String) => Call::Snapshots { id };
    list_dir(id: String, snapshot: String, path: String) => Call::ListDir { id, snapshot, path };
    unlock(id: String, password: String, create: Option<bool>)
        => Call::Unlock { id, password, create: create.unwrap_or(false) };
    save_key_file(id: String) => Call::SaveKeyFile { id };
    backup_now(id: String) => Call::BackupNow { id };
    cancel(id: String) => Call::Cancel { id };
    restore(id: String, snapshot: String, paths: Vec<String>, to: Option<String>, overwrite: bool)
        => Call::Restore { id, snapshot, paths, to, overwrite };
    get_profile(id: String) => Call::GetProfile { id };
    save_profile(id: String, profile: Value, create: bool) => Call::SaveProfile { id, profile, create };
    delete_profile(id: String) => Call::DeleteProfile { id };
    retention_preview(id: String, retention: Value) => Call::RetentionPreview { id, retention };
    exclude_suggestions(sources: Vec<String>) => Call::ExcludeSuggestions { sources };
    drives() => Call::Drives;
    update_snapshot(id: String, snapshot: String, name: Option<String>, set_name: bool, pinned: Option<bool>)
        => Call::UpdateSnapshot { id, snapshot, name, set_name, pinned };
    delete_snapshot(id: String, snapshot: String) => Call::DeleteSnapshot { id, snapshot };
    compare(id: String, old: String, new: Option<String>) => Call::Compare { id, old, new };
    runs(id: String, limit: usize) => Call::Runs { id, limit };
    run_events(id: String, run_id: String) => Call::RunEvents { id, run_id };
    prune(id: String, dry_run: bool) => Call::Prune { id, dry_run };
    check(id: String, read_data: bool) => Call::Check { id, read_data };
}

// Calls that aren't about an existing profile say which scope they're for.

#[tauri::command]
pub async fn new_profile(state: St<'_>, system: Option<bool>) -> Result<Value> {
    route(state, Call::NewProfile, system.unwrap_or(false)).await
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
        let mut other = json!("etc");
        qualify("suggest_id", &mut other);
        assert_eq!(other, "etc", "suggested ids stay plain");
    }
}
