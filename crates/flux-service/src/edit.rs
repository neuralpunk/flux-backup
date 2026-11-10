//! Profile editing, retention previews, exclusion suggestions, drive detection and target
//! tests: what the profile editor needs.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use flux_core::profile::{PasswordSource, Retention, Target, has_errors};
use flux_core::{Issue, Profile, ProfileId, ProfileStore, Scope, Severity, sys, target_check};
use flux_runner::schedule;

use crate::{ApiError, Result, Service};

/// Back In Time's profiles (from your `~/.config/backintime/config`), each as a new Flux
/// profile to start from. None for system profiles: Back In Time's root setup is its own.
pub fn backintime_impl(state: &Service) -> Result<Vec<flux_compat::backintime::BitProfile>> {
    if state.scope() == Scope::System {
        return Ok(Vec::new());
    }
    let Some(home) = sys::home_dir() else {
        return Ok(Vec::new());
    };
    let base = new_profile_impl(state)?.profile;
    flux_compat::backintime::read(&home.join(flux_compat::backintime::CONFIG), &base)
        .map_err(|e| ApiError::new("error", format!("reading Back In Time's settings: {e}")))
}

#[derive(Serialize)]
pub struct Editable {
    id: String,
    pub profile: Profile,
    issues: Vec<Issue>,
}

pub fn get_profile_impl(state: &Service, id: String) -> Result<Editable> {
    let (id, profile) = state.load(&id)?;
    Ok(Editable {
        id: id.to_string(),
        issues: validate(state, &profile),
        profile,
    })
}

fn validate(state: &Service, p: &Profile) -> Vec<Issue> {
    let mut issues = p.validate_in(state.scope());
    // Not an error (a drive that's unplugged today is fine), but say so before the first
    // backup tries.
    if let Some(why) = schedule::Scheduler::new(&state.paths).unsupported(p) {
        issues.push(Issue {
            severity: Severity::Warning,
            field: "schedule.on_connect".into(),
            message: why,
        });
    }
    if let Some((why, drive)) = crate::views::target_unreachable(p) {
        issues.push(Issue {
            severity: Severity::Warning,
            field: "target.connected".into(),
            message: if drive {
                format!("{why} Backups can't run until it's plugged in.")
            } else {
                why
            },
        });
    }
    issues
}

/// A starting point for a new profile: your home folder, or for system profiles the
/// machine's configuration and root's files.
pub fn new_profile_impl(state: &Service) -> Result<Editable> {
    if state.scope() == Scope::System {
        let mut p = Profile::new(
            "System settings",
            Target::Local {
                path: PathBuf::new(),
                fs_uuid: None,
            },
            vec!["/etc".into(), "/root".into(), "/usr/local".into()],
        );
        p.sources.exclude = vec!["/root/.cache/".into(), "*.tmp".into()];
        p.schedule.trigger = flux_core::profile::Trigger::Calendar;
        p.schedule.on_calendar = Some("daily".into());
        // Desktop notifications need a user session; root has none.
        p.options.notify = false;
        return Ok(Editable {
            id: String::new(),
            issues: Vec::new(),
            profile: p,
        });
    }
    let home = sys::home_dir().unwrap_or_else(|| PathBuf::from("/home"));
    let mut p = Profile::new(
        "Home folder",
        Target::Local {
            path: PathBuf::new(),
            fs_uuid: None,
        },
        vec![home.clone()],
    );
    p.sources.exclude = crate::prefs::new_profile_excludes(state);
    p.schedule.trigger = flux_core::profile::Trigger::Calendar;
    p.schedule.on_calendar = Some("daily".into());
    Ok(Editable {
        id: String::new(),
        issues: Vec::new(),
        profile: p,
    })
}

#[derive(Serialize)]
pub struct SaveResult {
    id: String,
    issues: Vec<Issue>,
    saved: bool,
    /// What happened to the timer, for a toast.
    schedule: Option<String>,
}

pub(crate) fn parse(profile: serde_json::Value) -> std::result::Result<Profile, Vec<Issue>> {
    serde_json::from_value::<Profile>(profile).map_err(|e| {
        vec![Issue {
            severity: Severity::Error,
            field: String::new(),
            message: e.to_string(),
        }]
    })
}

pub fn validate_profile_impl(state: &Service, profile: serde_json::Value) -> Result<Vec<Issue>> {
    Ok(match parse(profile) {
        Ok(p) => validate(state, &p),
        Err(issues) => issues,
    })
}

/// Save (create when `create`) a profile, then make its timer match.
pub fn save_profile_impl(
    state: &Service,
    id: String,
    profile: serde_json::Value,
    create: bool,
) -> Result<SaveResult> {
    let p = match parse(profile) {
        Ok(p) => p,
        Err(issues) => {
            return Ok(SaveResult {
                id,
                issues,
                saved: false,
                schedule: None,
            });
        }
    };
    let id = ProfileId::new(id).map_err(|e| ApiError::new("error", e.to_string()))?;
    let issues = validate(state, &p);
    if has_errors(&issues) {
        return Ok(SaveResult {
            id: id.to_string(),
            issues,
            saved: false,
            schedule: None,
        });
    }
    let store = ProfileStore::new(&state.paths);
    if create {
        store.create(&id, &p)?;
    } else {
        store.save(&id, &p)?;
    }
    state.forget_cached(&id);
    let schedule = apply_timer(state, &id, &p);
    Ok(SaveResult {
        id: id.to_string(),
        issues,
        saved: true,
        schedule,
    })
}

/// Delete a profile and its timer. Backups already made stay where they are.
pub fn delete_profile_impl(state: &Service, id: String) -> Result<()> {
    let id = ProfileId::new(id).map_err(|e| ApiError::new("error", e.to_string()))?;
    let _ = schedule::Scheduler::new(&state.paths).remove(&id);
    ProfileStore::new(&state.paths).delete(&id)?;
    flux_runner::index::remove(&state.paths, &id);
    state.forget_cached(&id);
    Ok(())
}

/// Save the Vault password the app holds for `id` into a key file only the owner can
/// read (`<config>/keys/<id>`), and point the profile at it, so scheduled backups can
/// open the Vault without anyone typing. Meant for system profiles, whose timers run
/// without a logged-in user. Returns the file's path.
pub fn save_key_file_impl(state: &Service, id: String) -> Result<String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let (pid, mut p) = state.load(&id)?;
    if p.password_source().is_none() {
        return Err(ApiError::new(
            "error",
            "only encrypted Vaults and Borg repositories have a password",
        ));
    }
    let password = state
        .password(&pid)
        .ok_or_else(|| ApiError::new("password_required", "Unlock the backups first."))?;
    let dir = state.paths.config_dir.join("keys");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let path = dir.join(pid.as_str());
    let tmp = dir.join(format!(".{pid}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(password.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    let source = PasswordSource::File { path: path.clone() };
    if p.engine == flux_core::Engine::Borg {
        p.borg.get_or_insert_with(Default::default).password_source = source;
    } else {
        p.vault.get_or_insert_with(Default::default).password_source = source;
    }
    ProfileStore::new(&state.paths).save(&pid, &p)?;
    state.forget_cached(&pid);
    Ok(path.display().to_string())
}

/// A free profile id derived from a name.
pub fn suggest_id_impl(state: &Service, name: String) -> Result<String> {
    let base = ProfileId::slugify(&name)
        .map(|i| i.to_string())
        .unwrap_or_else(|| "profile".into());
    let store = ProfileStore::new(&state.paths);
    let mut id = base.clone();
    let mut n = 2;
    while store
        .exists(&ProfileId::new(id.clone()).map_err(|e| ApiError::new("error", e.to_string()))?)
    {
        id = format!("{base}-{n}");
        n += 1;
    }
    Ok(id)
}

#[cfg(test)]
impl SaveResult {
    pub fn saved_flag(&self) -> bool {
        self.saved
    }
}

#[cfg(test)]
impl PreviewRow {
    pub fn keep_flag(&self) -> bool {
        self.keep
    }
}

#[derive(Serialize)]
pub struct PreviewRow {
    id: String,
    time: jiff::Timestamp,
    keep: bool,
    reasons: Vec<&'static str>,
    expired: bool,
    name: Option<String>,
    pinned: bool,
}

/// What a retention policy would keep and remove of a profile's current snapshots.
/// Reads only; nothing is deleted.
pub fn retention_preview_impl(
    state: &Service,
    id: String,
    retention: serde_json::Value,
) -> Result<Vec<PreviewRow>> {
    let policy: Retention =
        serde_json::from_value(retention).map_err(|e| ApiError::new("error", e.to_string()))?;
    let (pid, p) = state.load(&id)?;
    let snaps = match state.open(&pid, &p) {
        Ok(r) => r.snapshots()?,
        Err(e) if e.code() == "empty" => Vec::new(),
        Err(e) => return Err(e),
    };
    let infos: Vec<_> = snaps.iter().map(flux_vault::gc::info).collect();
    let decisions = flux_core::retention::evaluate(
        &infos,
        &policy,
        jiff::Timestamp::now(),
        &jiff::tz::TimeZone::system(),
    );
    Ok(decisions
        .into_iter()
        .map(|d| {
            let s = snaps.iter().find(|s| s.id == d.id);
            PreviewRow {
                reasons: d.reasons.iter().map(|r| r.label()).collect(),
                name: s.and_then(|s| s.name.clone()),
                pinned: s.is_some_and(|s| s.pinned),
                id: d.id,
                time: d.time,
                keep: d.keep,
                expired: d.expired,
            }
        })
        .collect())
}

#[derive(Serialize)]
pub struct Suggestion {
    pattern: String,
    label: String,
    why: &'static str,
    /// Bytes found under the profile's sources (for path-like suggestions), if measured.
    size: Option<u64>,
}

/// Well-known things not worth backing up, with their size under `sources` when it can be
/// measured quickly.
pub fn exclude_suggestions_impl(_state: &Service, sources: Vec<String>) -> Result<Vec<Suggestion>> {
    const ALL: &[(&str, &str, &str)] = &[
        ("~/.cache/", "Application caches", "rebuilt automatically"),
        (
            "~/.local/share/Trash/",
            "Trash",
            "files you already deleted",
        ),
        ("node_modules/", "node_modules", "reinstalled with npm/pnpm"),
        ("**/target/debug/", "Rust debug builds", "rebuilt by cargo"),
        (
            "**/target/release/",
            "Rust release builds",
            "rebuilt by cargo",
        ),
        ("__pycache__/", "Python bytecode", "regenerated by Python"),
        (
            ".venv/",
            "Python virtualenvs",
            "recreated from requirements",
        ),
        (
            "~/.local/share/Steam/steamapps/",
            "Steam games",
            "downloadable again",
        ),
        (
            "~/.var/app/*/cache/",
            "Flatpak app caches",
            "rebuilt automatically",
        ),
        (
            "~/.thumbnails/",
            "Thumbnails",
            "regenerated by file managers",
        ),
        ("~/Downloads/", "Downloads", "often re-downloadable"),
        ("*.tmp", "Temporary files", "short-lived by definition"),
        (
            "~/.local/share/containers/",
            "Container images",
            "pullable again",
        ),
    ];
    let home = sys::home_dir();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut out = Vec::new();
    for (pattern, label, why) in ALL {
        let size = pattern
            .strip_prefix("~/")
            .and_then(|rest| home.as_ref().map(|h| h.join(rest.trim_end_matches('/'))))
            .filter(|p| !p.as_os_str().to_string_lossy().contains('*'))
            .filter(|p| sources.iter().any(|s| p.starts_with(s)))
            .filter(|p| p.is_dir())
            .and_then(|p| du(&p, deadline));
        out.push(Suggestion {
            pattern: (*pattern).into(),
            label: (*label).into(),
            why,
            size,
        });
    }
    Ok(out)
}

/// Apparent size of a tree, giving up at `deadline`.
fn du(dir: &Path, deadline: Instant) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if Instant::now() > deadline {
            return None;
        }
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.blocks() * 512;
            }
        }
    }
    Some(total)
}

#[derive(Serialize)]
pub struct Drive {
    mount_point: PathBuf,
    label: String,
    fs_uuid: Option<String>,
    fs_type: String,
    free: Option<u64>,
    total: Option<u64>,
}

/// Mounted removable or extra drives, for the target picker.
pub fn drives_impl(_state: &Service) -> Result<Vec<Drive>> {
    let by_uuid: Vec<(String, PathBuf)> = std::fs::read_dir("/dev/disk/by-uuid")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    Some((
                        e.file_name().to_string_lossy().into_owned(),
                        std::fs::canonicalize(e.path()).ok()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for m in sys::mounts() {
        let mp = m.mount_point.to_string_lossy();
        let interesting = ["/run/media/", "/media/", "/mnt/"]
            .iter()
            .any(|r| mp.starts_with(r));
        if !interesting {
            continue;
        }
        let dev = std::fs::canonicalize(&m.source).ok();
        let fs_uuid = dev.and_then(|d| {
            by_uuid
                .iter()
                .find(|(_, p)| *p == d)
                .map(|(u, _)| u.clone())
        });
        let vfs = rustix::fs::statvfs(&m.mount_point).ok();
        out.push(Drive {
            label: m
                .mount_point
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            fs_type: m.fs_type,
            fs_uuid,
            free: vfs.as_ref().map(|v| v.f_bavail * v.f_frsize),
            total: vfs.as_ref().map(|v| v.f_blocks * v.f_frsize),
            mount_point: m.mount_point,
        });
    }
    Ok(out)
}

#[derive(Serialize, Deserialize)]
pub struct TargetTest {
    ok: bool,
    messages: Vec<String>,
}

/// Check that a target is usable, without prompting: local drive presence, or SSH login
/// (batch mode), SFTP, write access and free space.
pub fn test_target_impl(state: &Service, profile: serde_json::Value) -> Result<TargetTest> {
    let p = parse(profile).map_err(|i| ApiError::new("error", i[0].message.clone()))?;
    let mut messages = Vec::new();
    if p.engine.is_compat() {
        let tool = if p.engine == flux_core::Engine::Borg {
            flux_compat::tools::borg_status()
        } else {
            flux_compat::tools::rsync_status()
        };
        match (&tool.version, tool.supported) {
            (Some(v), true) => messages.push(format!("{} {v} is installed", tool.tool)),
            _ => {
                return Ok(TargetTest {
                    ok: false,
                    messages: vec![tool.hint.unwrap_or_default()],
                });
            }
        }
    }
    match &p.target {
        Target::Local { path, fs_uuid } => {
            match target_check::check_local_target(path, fs_uuid.as_deref()) {
                Ok(()) => {
                    let free = rustix::fs::statvfs(path)
                        .ok()
                        .map(|v| v.f_bavail * v.f_frsize);
                    messages.push(format!("{} is ready", path.display()));
                    if let Some(f) = free {
                        messages.push(format!("{} free", flux_core::ByteSize(f).display_approx()));
                    }
                    Ok(TargetTest { ok: true, messages })
                }
                Err(e) => Ok(TargetTest {
                    ok: false,
                    messages: vec![e.to_string()],
                }),
            }
        }
        Target::Ssh { path, .. } => {
            let Some(t) =
                flux_transport::SshTarget::from_target(&p.target, &state.paths.runtime_dir, true)
            else {
                return Ok(TargetTest {
                    ok: false,
                    messages: vec!["not an SSH target".into()],
                });
            };
            let result = (|| -> std::result::Result<(), String> {
                t.run("true").map_err(|e| format!("{e}. If this server needs a password or a new host key, run `flux ssh test` in a terminal once."))?;
                messages.push(format!("logged in to {}", t.destination()));
                if matches!(
                    p.engine,
                    flux_core::Engine::Mirror | flux_core::Engine::Rsync
                ) {
                    t.run("command -v rsync").map_err(|_| {
                        format!(
                            "rsync is not installed on the server ({} over SSH needs it)",
                            p.engine.label()
                        )
                    })?;
                    messages.push("rsync is available".into());
                }
                if p.engine == flux_core::Engine::Borg {
                    let remote = p
                        .borg
                        .as_ref()
                        .and_then(|b| b.remote_path.clone())
                        .unwrap_or_else(|| "borg".into());
                    t.run(&format!(
                        "command -v {}",
                        flux_transport::shell_quote(&remote)
                    ))
                    .map_err(|_| {
                        "borg is not installed on the server (the Borg engine over SSH needs it)"
                            .to_owned()
                    })?;
                    messages.push("borg is available on the server".into());
                }
                let mut s = t.sftp().map_err(|e| e.to_string())?;
                let dir = path.display().to_string();
                s.mkdir_p(&dir, 0o700)
                    .map_err(|e| format!("cannot create {dir}: {e}"))?;
                let probe = format!("{dir}/.flux-write-test");
                s.write_new(&probe, b"ok", 0o600)
                    .map_err(|e| format!("cannot write in {dir}: {e}"))?;
                let _ = s.remove(&probe);
                messages.push(format!("{dir} is writable"));
                if let Ok(v) = s.statvfs(&dir) {
                    messages.push(format!(
                        "{} free",
                        flux_core::ByteSize(v.free_bytes()).display_approx()
                    ));
                }
                Ok(())
            })();
            match result {
                Ok(()) => Ok(TargetTest { ok: true, messages }),
                Err(e) => {
                    messages.push(e);
                    Ok(TargetTest {
                        ok: false,
                        messages,
                    })
                }
            }
        }
    }
}

/// Install, update or remove the profile's systemd timer to match its schedule. Returns
/// what happened, for people (`None`: no timer involved).
pub fn apply_timer(state: &Service, id: &ProfileId, p: &Profile) -> Option<String> {
    let sd = schedule::Scheduler::new(&state.paths);
    let needs_timer =
        p.schedule.trigger != flux_core::profile::Trigger::Manual || p.schedule.on_connect;
    if !needs_timer && !sd.installed_ids().contains(id) {
        return None;
    }
    Some(match sd.apply(id, p, &state.unit_env()) {
        Ok(a) if a.changed() => format!("Timer updated: {}", schedule::summary(p)),
        Ok(_) => "Timer unchanged".into(),
        Err(e) => format!("Saved, but the timer couldn't be updated: {e}"),
    })
}

/// The formats a profile can be exported in.
pub fn export_formats_impl() -> Vec<flux_compat::export::FormatInfo> {
    flux_compat::export::Format::ALL
        .into_iter()
        .map(|f| f.info())
        .collect()
}

/// A profile exported as `format` (see `flux_compat::export`).
pub fn export_profile_impl(
    state: &Service,
    id: String,
    format: String,
) -> Result<flux_compat::export::Export> {
    let (pid, p) = state.load(&id)?;
    let f = flux_compat::export::Format::parse(&format)
        .ok_or_else(|| ApiError::new("bad_request", format!("unknown export format {format:?}")))?;
    Ok(flux_compat::export::export(&pid, &p, f))
}

#[derive(Serialize)]
pub struct Imported {
    id: String,
    issues: Vec<flux_core::Issue>,
}

/// Import a Flux profile. Its id: `id`, else the one it was exported under, else one
/// from its name; made unique (`-2`, `-3`…) rather than replacing a profile. Problems
/// (a drive that has another name here…) don't stop the import; they're returned so the
/// app can open the profile's settings.
pub fn import_profile_impl(state: &Service, toml: String, id: Option<String>) -> Result<Imported> {
    let (exported, p) = flux_compat::export::parse_flux(&toml)
        .map_err(|e| ApiError::new("error", format!("This isn't a Flux profile: {e}")))?;
    let base = id
        .or(exported)
        .and_then(|i| ProfileId::new(i).ok())
        .or_else(|| ProfileId::slugify(&p.name))
        .ok_or_else(|| ApiError::new("error", "The profile has no usable name."))?;
    let store = ProfileStore::new(&state.paths);
    let mut pid = base.clone();
    let mut n = 2;
    while store.exists(&pid) {
        pid = ProfileId::new(format!(
            "{}-{n}",
            &base.as_str()[..base.as_str().len().min(60)]
        ))?;
        n += 1;
    }
    store.create(&pid, &p)?;
    Ok(Imported {
        id: pid.to_string(),
        issues: p.validate_in(state.scope()),
    })
}

/// Add a destination to a saved profile, right away (not through the editor's Save): the
/// profile is validated and saved, and for an encrypted Vault or Borg repository the
/// password is set up at once, as for a new backup, so the next run asks nobody.
/// Returns the saved profile.
pub fn add_destination_impl(
    state: &Service,
    id: String,
    destination: serde_json::Value,
    password: Option<String>,
) -> Result<Profile> {
    let (pid, mut p) = state.load(&id)?;
    if pid.split_destination().is_some() {
        return Err(ApiError::new(
            "error",
            "Add destinations to the backup itself.",
        ));
    }
    let d: flux_core::profile::Destination = serde_json::from_value(destination)
        .map_err(|e| ApiError::new("invalid", format!("destination: {e}")))?;
    if p.destinations.iter().any(|x| x.name == d.name) {
        return Err(ApiError::new(
            "invalid",
            format!("This backup already has a destination named “{}”.", d.name),
        ));
    }
    let name = d.name.clone();
    p.destinations.push(d);
    if let Some(i) = validate(state, &p)
        .into_iter()
        .find(|i| i.severity == Severity::Error && i.field.starts_with("destinations"))
    {
        return Err(ApiError::new("invalid", i.message));
    }
    ProfileStore::new(&state.paths).save(&pid, &p)?;
    let did = pid
        .with_destination(&name)
        .map_err(|e| ApiError::new("error", e.to_string()))?;
    if let Some(pw) = password.filter(|pw| !pw.is_empty())
        && let Err(e) = crate::views::set_password_impl(state, did.to_string(), pw)
    {
        return Err(ApiError::new(
            e.code(),
            format!("The destination was added, but its password couldn't be set up: {e}"),
        ));
    }
    state.forget_cached(&pid);
    Ok(ProfileStore::new(&state.paths).load(&pid)?)
}

/// Remove a destination from a profile. The backups made there stay where they are.
/// Returns the saved profile.
pub fn remove_destination_impl(state: &Service, id: String, name: String) -> Result<Profile> {
    let (pid, mut p) = state.load(&id)?;
    let before = p.destinations.len();
    p.destinations.retain(|d| d.name != name);
    if p.destinations.len() == before {
        return Err(ApiError::new(
            "error",
            format!("This backup has no destination “{name}”."),
        ));
    }
    let store = ProfileStore::new(&state.paths);
    store.save(&pid, &p)?;
    Ok(store.load(&pid)?)
}

/// What the rule tester shows: is `path` backed up by `profile` (the editor's draft),
/// and why.
#[derive(Serialize)]
pub struct Explained {
    pub path: String,
    pub backed_up: bool,
    pub why: flux_fs::coverage::Why,
    pub text: String,
}
fn rules_of(profile: serde_json::Value) -> Result<flux_fs::coverage::Rules> {
    let p = parse(profile).map_err(|i| ApiError::new("invalid", i[0].message.clone()))?;
    flux_fs::coverage::Rules::of(&p).map_err(|e| ApiError::new("invalid", e.to_string()))
}
fn absolute(path: &str) -> Result<std::path::PathBuf> {
    let expanded = match (path.strip_prefix('~'), flux_core::sys::home_dir()) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", h.display())
        }
        _ => path.to_owned(),
    };
    let p = std::path::PathBuf::from(&expanded);
    if !p.is_absolute() {
        return Err(ApiError::new(
            "invalid",
            "Type a full path, starting with / or ~/.",
        ));
    }
    Ok(p)
}
pub fn explain_impl(profile: serde_json::Value, path: String) -> Result<Explained> {
    let rules = rules_of(profile)?;
    let abs = absolute(path.trim())?;
    let why = flux_fs::coverage::explain(&rules, &abs);
    Ok(Explained {
        text: why.text(&abs),
        backed_up: why.included(),
        path: abs.display().to_string(),
        why,
    })
}
/// A folder's items, each marked backed up or left out, with totals (counted for up to
/// 10 seconds).
pub fn coverage_impl(
    profile: serde_json::Value,
    dir: String,
) -> Result<flux_fs::coverage::Coverage> {
    let rules = rules_of(profile)?;
    let abs = absolute(dir.trim())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    flux_fs::coverage::list(&rules, &abs, Some(deadline))
        .map_err(|e| ApiError::new("error", format!("{}: {e}", abs.display())))
}
/// The templates a new backup can start from.
pub fn templates_impl(state: &Service) -> Vec<flux_runner::templates::Template> {
    flux_runner::templates::list(&state.paths)
}
/// A new backup, from template `template`.
pub fn new_from_template_impl(state: &Service, template: String) -> Result<Editable> {
    let base = new_profile_impl(state)?;
    let home = sys::home_dir().unwrap_or_else(|| PathBuf::from("/root"));
    let p = flux_runner::templates::apply(&state.paths, &template, base.profile, &home)
        .map_err(|e| ApiError::new("error", e))?;
    Ok(Editable {
        id: String::new(),
        issues: Vec::new(),
        profile: p,
    })
}
/// Save a backup's settings as a template (without where it goes). Returns the
/// templates.
pub fn save_template_impl(
    state: &Service,
    id: String,
    name: String,
) -> Result<Vec<flux_runner::templates::Template>> {
    let (_, p) = state.load(&id)?;
    flux_runner::templates::save(&state.paths, &name, &p)
        .map_err(|e| ApiError::new("invalid", e))?;
    Ok(templates_impl(state))
}
/// Remove one of your templates. Returns the templates.
pub fn delete_template_impl(
    state: &Service,
    template: String,
) -> Result<Vec<flux_runner::templates::Template>> {
    flux_runner::templates::delete(&state.paths, &template)
        .map_err(|e| ApiError::new("error", e))?;
    Ok(templates_impl(state))
}

#[cfg(test)]
mod destination_tests {
    use super::*;
    use flux_core::Paths;

    #[test]
    fn add_and_remove_an_encrypted_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        for d in ["src", "main", "usb"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        let state = Service::new(Paths::under(t.join("home"), Scope::User));
        let id = ProfileId::new("home").unwrap();
        let p = Profile::new(
            "Home",
            Target::Local {
                path: t.join("main"),
                fs_uuid: None,
            },
            vec![t.join("src")],
        );
        ProfileStore::new(&state.paths).create(&id, &p).unwrap();
        let key = t.join("keys/usb.pass");
        let dest = serde_json::json!({
            "name": "usb",
            "label": "USB stick",
            "engine": "vault",
            "target": { "kind": "local", "path": t.join("usb") },
            "vault": {
                "password_source": { "from": "file", "path": key },
                "hashing": { "level": "custom", "memory_mib": 8, "passes": 1, "threads": 1 },
            },
        });
        let saved =
            add_destination_impl(&state, "home".into(), dest.clone(), Some("pw".into())).unwrap();
        assert_eq!(saved.destinations.len(), 1);
        // The Vault exists at once, its password kept where the destination says.
        assert!(t.join("usb/flux-vault/config").exists());
        assert_eq!(std::fs::read_to_string(&key).unwrap().trim(), "pw");
        // Browsing it as a profile of its own works (no snapshots yet).
        let e = crate::views::snapshots_impl(&state, "home+usb".into());
        assert!(
            e.is_ok() || e.as_ref().err().is_some_and(|e| e.code() == "empty"),
            "{e:?}"
        );
        // The same name again, or the main destination's place: refused.
        assert_eq!(
            add_destination_impl(&state, "home".into(), dest, None)
                .unwrap_err()
                .code(),
            "invalid"
        );
        let same_place = serde_json::json!({ "name": "again", "engine": "mirror", "target": { "kind": "local", "path": t.join("main") } });
        assert_eq!(
            add_destination_impl(&state, "home".into(), same_place, None)
                .unwrap_err()
                .code(),
            "invalid"
        );
        // A destination can't have destinations.
        assert!(
            add_destination_impl(&state, "home+usb".into(), serde_json::json!({}), None).is_err()
        );

        let saved = remove_destination_impl(&state, "home".into(), "usb".into()).unwrap();
        assert!(saved.destinations.is_empty());
        assert!(t.join("usb/flux-vault/config").exists(), "its backups stay");
        assert!(remove_destination_impl(&state, "home".into(), "usb".into()).is_err());
    }
}
#[cfg(test)]
mod explain_tests {
    use super::*;

    #[test]
    fn the_draft_decides() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        std::fs::create_dir_all(src.join("build")).unwrap();
        std::fs::write(src.join("build/out.o"), "o").unwrap();
        std::fs::write(src.join("a.c"), "c").unwrap();
        let mut p = Profile::new(
            "T",
            flux_core::profile::Target::Local {
                path: t.path().join("tgt"),
                fs_uuid: None,
            },
            vec![src.clone()],
        );
        let draft = |p: &Profile| serde_json::to_value(p).unwrap();
        let out = src.join("build/out.o").display().to_string();
        assert!(explain_impl(draft(&p), out.clone()).unwrap().backed_up);
        // An exclusion typed in the editor, not saved yet.
        p.sources.exclude = vec!["build/".into()];
        let e = explain_impl(draft(&p), out).unwrap();
        assert!(!e.backed_up && e.text.contains("build/"), "{}", e.text);
        let c = coverage_impl(draft(&p), src.display().to_string()).unwrap();
        let build = c.items.iter().find(|i| i.name == "build").unwrap();
        assert!(!build.backed_up && build.files_out == 1);
        assert!(explain_impl(draft(&p), "relative/path".into()).is_err());
    }
}
#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn new_backups_from_templates() {
        let t = tempfile::tempdir().unwrap();
        let paths = flux_core::Paths::under(t.path().join("home"), Scope::User);
        let state = Service::new(paths.clone());
        let e = new_from_template_impl(&state, "development".into()).unwrap();
        assert_eq!(e.profile.name, "Projects");
        assert!(e.profile.sources.exclude.contains(&"target/".to_owned()));
        let mut p = e.profile.clone();
        p.target = Target::Local {
            path: t.path().join("tgt"),
            fs_uuid: None,
        };
        let id = ProfileId::new("dev").unwrap();
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let after = save_template_impl(&state, "dev".into(), "My dev setup".into()).unwrap();
        let mine = after.iter().find(|t| t.own).unwrap();
        let again = new_from_template_impl(&state, mine.id.clone()).unwrap();
        assert_eq!(again.profile.sources.exclude, p.sources.exclude);
        assert_eq!(
            again.profile.target,
            new_profile_impl(&state).unwrap().profile.target
        );
        assert!(
            delete_template_impl(&state, mine.id.clone())
                .unwrap()
                .iter()
                .all(|t| !t.own)
        );
        assert!(new_from_template_impl(&state, "own:gone".into()).is_err());
    }
}
