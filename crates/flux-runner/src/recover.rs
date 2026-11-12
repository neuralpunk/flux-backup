//! Recovering backups on a new computer.
//!
//! After each backup, a copy of the profile's settings is kept with its backups
//! ([`save_copy`]): encrypted inside a Vault, as `flux-profile.toml` in the backup's folder
//! for Mirror, rsync and Borg. On another computer, [`find`] lists the backups on a drive
//! or a server (with the saved settings when it can read them), and [`recover`] makes a
//! profile that reaches them again: the same folders, exclusions and rules, and an
//! [`Origin`] so they're found, listed and continued under the names they were made with.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_compat::export::{SAVED_PROFILE, parse_recovery_copy, recovery_copy};
use flux_compat::store::Store;
use flux_core::profile::{Engine, KeptAs, Origin, Profile, Target, Trigger};
use flux_core::{Paths, ProfileId, sys};
use flux_transport::{SftpPool, SshTarget};

/// What was last kept with the backups, for the readiness checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyState {
    pub saved: Timestamp,
    /// Of the copy's text: it's current while the profile's copy has the same digest.
    pub digest: String,
}

fn state_path(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths.state_dir.join("recovery").join(format!("{id}.json"))
}

pub fn load_state(paths: &Paths, id: &ProfileId) -> Option<CopyState> {
    std::fs::read(state_path(paths, id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

fn save_state(paths: &Paths, id: &ProfileId, digest: String) {
    let path = state_path(paths, id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let s = CopyState {
        saved: Timestamp::now(),
        digest,
    };
    if let Ok(json) = serde_json::to_vec_pretty(&s) {
        let _ = flux_core::store::write_atomic(&path, &json, 0o600);
    }
}

/// FNV-1a of `text`, as hex: enough to tell whether the copy kept is the current one.
pub fn digest(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Is the copy kept with the backups the profile as it is now?
pub fn copy_current(paths: &Paths, id: &ProfileId, p: &Profile) -> Option<bool> {
    load_state(paths, id).map(|s| s.digest == digest(&recovery_copy(id, p)))
}

/// The folder a Mirror, rsync or Borg backup keeps its files in (on the drive or server).
fn folder(id: &ProfileId, p: &Profile) -> Option<String> {
    let k = p.kept_as(id);
    match (p.engine, &p.target) {
        (Engine::Mirror, Target::Local { path, .. }) => Some(
            flux_mirror::repo::MirrorRepo::locate(path, &k.host, &k.user, &k.profile)
                .display()
                .to_string(),
        ),
        (Engine::Mirror, Target::Ssh { path, .. }) => Some(
            flux_mirror::remote::RemoteMirror::locate(path, &k.host, &k.user, &k.profile)
                .to_string(),
        ),
        (Engine::Rsync, _) => Some(flux_compat::rsync::repo_location(id, p)),
        (Engine::Borg, _) => Some(flux_compat::borg::folder_location(id, p)),
        (Engine::Vault, _) => None,
    }
}

fn pool_for(paths: &Paths, target: &Target, batch: bool) -> Result<Arc<SftpPool>, String> {
    SshTarget::from_target(target, &paths.runtime_dir, batch)
        .map(|t| Arc::new(SftpPool::new(t, 2)))
        .ok_or_else(|| "not a server".to_owned())
}

/// After a backup: keep the profile's settings with its backups, unless the copy there is
/// already current. A Vault's copy is kept by [`save_copy_in_vault`] while it's open.
pub fn save_copy(paths: &Paths, id: &ProfileId, p: &Profile, batch: bool) -> Result<bool, String> {
    let Some(folder) = folder(id, p) else {
        return Ok(false);
    };
    let text = recovery_copy(id, p);
    let d = digest(&text);
    if load_state(paths, id).is_some_and(|s| s.digest == d) {
        return Ok(false);
    }
    let store = match &p.target {
        Target::Local { .. } => Store::local(PathBuf::from(&folder)),
        Target::Ssh { .. } => Store::remote(pool_for(paths, &p.target, batch)?, folder),
    };
    store
        .write(SAVED_PROFILE, text.as_bytes(), 0o600)
        .map_err(|e| e.to_string())?;
    save_state(paths, id, d);
    Ok(true)
}

/// The same for a Vault, through the repository a backup has open.
pub fn save_copy_in_vault(
    paths: &Paths,
    id: &ProfileId,
    p: &Profile,
    repo: &flux_vault::VaultRepo,
    owner: &flux_vault::repo::Owner,
) -> Result<bool, String> {
    let text = recovery_copy(id, p);
    let d = digest(&text);
    if load_state(paths, id).is_some_and(|s| s.digest == d) {
        return Ok(false);
    }
    repo.save_profile(owner, &text).map_err(|e| e.to_string())?;
    save_state(paths, id, d);
    Ok(true)
}

/// A backup found on a drive or a server.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub engine: Engine,
    /// The names it's kept under: the computer, user and profile that made it.
    pub kept: KeptAs,
    /// Its folder (or the Vault's), for messages.
    pub location: String,
    /// The saved settings' name for it, or the profile id.
    pub name: String,
    /// The folders it backs up (from the saved settings, or the newest snapshot).
    pub sources: Vec<PathBuf>,
    /// Its settings were kept with it.
    pub saved: bool,
    pub snapshots: Option<usize>,
    pub newest: Option<Timestamp>,
    /// Made on this computer by this user (an existing profile may already use it).
    pub here: bool,
    /// The saved settings (not sent to the app: read again when recovering).
    #[serde(skip)]
    pub profile: Option<Profile>,
    /// A Vault: whether it's encrypted, as found.
    #[serde(skip)]
    pub vault_encrypted: Option<bool>,
}

/// What [`find`] saw.
#[derive(Debug, Clone, Serialize)]
pub struct Search {
    pub found: Vec<Found>,
    /// There's an encrypted Vault, and no (or a wrong) password was given.
    pub vault_locked: bool,
    /// The password was given but doesn't open the Vault.
    pub wrong_password: bool,
}

const ROOTS: [&str; 4] = ["flux-vault", "flux-backup", "flux-rsync", "flux-borg"];

/// Where to look: `target` itself, or its parent when it's one of Flux's own folders
/// (someone picked `/run/media/me/Drive/flux-vault`).
fn search_root(target: &Target) -> Target {
    let up = |path: &Path| match path.file_name().and_then(|n| n.to_str()) {
        Some(n) if ROOTS.contains(&n) => path.parent().map(Path::to_path_buf),
        _ => None,
    };
    let mut t = target.clone();
    match &mut t {
        Target::Local { path, .. } | Target::Ssh { path, .. } => {
            if let Some(p) = up(path) {
                *path = p;
            }
        }
    }
    t
}

/// Look for Flux backups at `target` (a drive's folder, or a folder on a server). An
/// encrypted Vault's backups are listed when `password` opens it.
pub fn find(
    paths: &Paths,
    target: &Target,
    password: Option<&str>,
    batch: bool,
) -> Result<Search, String> {
    let target = search_root(target);
    let store = match &target {
        Target::Local { path, .. } => {
            if !path.is_dir() {
                return Err(format!("{} isn't a folder that's there", path.display()));
            }
            Store::local(path.clone())
        }
        Target::Ssh { path, .. } => {
            Store::remote(pool_for(paths, &target, batch)?, path.display().to_string())
        }
    };
    let mut out = Search {
        found: Vec::new(),
        vault_locked: false,
        wrong_password: false,
    };
    let (host, user) = (sys::hostname(), sys::username());
    let dirs = |rel: &str| -> Vec<String> {
        store
            .list(rel)
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, is_dir)| *is_dir && !n.starts_with('.'))
            .map(|(n, _)| n)
            .collect()
    };
    for (root, engine) in [
        ("flux-backup", Engine::Mirror),
        ("flux-rsync", Engine::Rsync),
        ("flux-borg", Engine::Borg),
    ] {
        for h in dirs(root) {
            for u in dirs(&format!("{root}/{h}")) {
                for prof in dirs(&format!("{root}/{h}/{u}")) {
                    let rel = format!("{root}/{h}/{u}/{prof}");
                    let is_repo = match engine {
                        Engine::Mirror => {
                            store.exists(&format!("{rel}/{}", flux_mirror::repo::REPO_MARKER))
                        }
                        Engine::Rsync => {
                            store.exists(&format!("{rel}/{}", flux_compat::rsync::MARKER))
                        }
                        _ => store.is_dir(&format!("{rel}/{}", flux_compat::borg::REPO_DIR)),
                    };
                    if !is_repo {
                        continue;
                    }
                    let saved = store
                        .read(&format!("{rel}/{SAVED_PROFILE}"))
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .and_then(|t| parse_recovery_copy(&t).ok())
                        .map(|(p, _)| p);
                    // Snapshots: one folder each (Mirror, rsync); Borg needs its password.
                    let snaps = match engine {
                        Engine::Borg => None,
                        _ => Some(
                            dirs(&format!("{rel}/snapshots"))
                                .into_iter()
                                .filter(|n| !n.starts_with('.'))
                                .collect::<Vec<_>>(),
                        ),
                    };
                    let kept = KeptAs {
                        host: h.clone(),
                        user: u.clone(),
                        profile: prof.clone(),
                    };
                    out.found.push(Found {
                        engine,
                        here: h == host && u == user,
                        location: store.path(&rel),
                        name: saved
                            .as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| prof.clone()),
                        sources: saved
                            .as_ref()
                            .map(|p| p.sources.paths.clone())
                            .unwrap_or_default(),
                        saved: saved.is_some(),
                        snapshots: snaps.as_ref().map(Vec::len),
                        newest: None,
                        kept,
                        profile: saved,
                        vault_encrypted: None,
                    });
                }
            }
        }
    }
    if store.exists("flux-vault/config") {
        let mut probe = Profile::new("", target.clone(), Vec::new());
        probe.engine = Engine::Vault;
        let pool = match &target {
            Target::Ssh { .. } => Some(pool_for(paths, &target, batch)?),
            Target::Local { .. } => None,
        };
        let backend = crate::repo::vault_backend(&probe, pool);
        let location = backend.describe();
        match flux_vault::VaultRepo::open_with(backend, password, None) {
            Ok(repo) => vault_found(&repo, &location, &host, &user, &mut out.found)?,
            Err(flux_vault::VaultError::PasswordRequired) => out.vault_locked = true,
            Err(flux_vault::VaultError::WrongPassword) => {
                out.vault_locked = true;
                out.wrong_password = true;
            }
            Err(e) => return Err(format!("the Vault at {location}: {e}")),
        }
    }
    out.found.sort_by(|a, b| {
        (a.here, &a.kept.host, &a.kept.user, &a.kept.profile).cmp(&(
            b.here,
            &b.kept.host,
            &b.kept.user,
            &b.kept.profile,
        ))
    });
    Ok(out)
}

/// The backups in an open Vault: one per owner of its snapshots (or of a saved profile).
fn vault_found(
    repo: &flux_vault::VaultRepo,
    location: &str,
    host: &str,
    user: &str,
    out: &mut Vec<Found>,
) -> Result<(), String> {
    let records = repo.records().map_err(|e| e.to_string())?;
    let saved = repo.saved_profiles().map_err(|e| e.to_string())?;
    let mut owners: Vec<flux_vault::repo::Owner> = Vec::new();
    for r in &records {
        let o = flux_vault::repo::Owner {
            profile: r.meta.profile.clone(),
            host: r.meta.host.clone(),
            user: r.meta.user.clone(),
        };
        if !owners.contains(&o) {
            owners.push(o);
        }
    }
    for (o, _) in &saved {
        if !owners.contains(o) {
            owners.push(o.clone());
        }
    }
    for o in owners {
        let mine: Vec<_> = records.iter().filter(|r| o.matches(&r.meta)).collect();
        let newest = mine.iter().map(|r| r.meta.started).max();
        let profile = saved
            .iter()
            .find(|(so, _)| *so == o)
            .and_then(|(_, t)| parse_recovery_copy(t).ok())
            .map(|(p, _)| p);
        let sources = match &profile {
            Some(p) => p.sources.paths.clone(),
            None => mine
                .iter()
                .max_by_key(|r| r.meta.started)
                .map(|r| r.meta.sources.clone())
                .unwrap_or_default(),
        };
        out.push(Found {
            engine: Engine::Vault,
            here: o.host == host && o.user == user,
            location: location.to_owned(),
            name: profile
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| o.profile.clone()),
            sources,
            saved: profile.is_some(),
            snapshots: Some(mine.len()),
            newest,
            kept: KeptAs {
                host: o.host,
                user: o.user,
                profile: o.profile,
            },
            profile,
            vault_encrypted: Some(repo.config.encrypted),
        });
    }
    Ok(())
}

/// How a recovered backup goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Then {
    /// Only to get files back: no scheduled backups until you choose.
    Restore,
    /// Carry on backing up from this computer, on the saved schedule.
    CarryOn,
}

/// The profile to recover `found` (found at `target`) with: its saved settings, or one
/// made from what the backups show, reaching them at `target`.
pub fn recovered_profile(found: &Found, target: &Target, then: Then) -> Profile {
    let base = search_root(target);
    let mut p = match &found.profile {
        Some(p) => p.clone(),
        None => {
            let mut p = Profile::new(found.name.clone(), base.clone(), found.sources.clone());
            p.engine = found.engine;
            if found.engine == Engine::Vault {
                p.vault = Some(Default::default());
            }
            if found.engine == Engine::Borg {
                p.borg = Some(Default::default());
            }
            p
        }
    };
    p.engine = found.engine;
    // A Vault is taken up as it was found, encrypted or not.
    if let (Engine::Vault, Some(enc)) = (found.engine, found.vault_encrypted) {
        let v = p.vault.get_or_insert_with(Default::default);
        v.encryption = enc;
        if !enc {
            v.hide_sizes = false;
            v.unlock_file = None;
        }
    }
    // Where it was found, keeping what the saved target knew (the key file, the upload
    // limit; its ssh options were dropped when it was read, as they can run commands).
    p.target = match (&p.target, &base) {
        (Target::Ssh { .. }, Target::Ssh { .. }) => {
            let mut t = p.target.clone();
            if let (
                Target::Ssh {
                    host,
                    user,
                    port,
                    path,
                    ..
                },
                Target::Ssh {
                    host: h,
                    user: u,
                    port: o,
                    path: pa,
                    ..
                },
            ) = (&mut t, &base)
            {
                (*host, *user, *port, *path) = (h.clone(), u.clone(), *o, pa.clone());
            }
            t
        }
        _ => base,
    };
    if let Target::Local { fs_uuid, .. } = &mut p.target {
        *fs_uuid = None;
    }
    // Other places it was copied to may be elsewhere now: add them again.
    p.destinations.clear();
    p.origin = Some(Origin {
        host: found.kept.host.clone(),
        user: found.kept.user.clone(),
        profile: found.kept.profile.clone(),
    });
    if then == Then::Restore {
        p.schedule.trigger = Trigger::Manual;
    }
    p
}

/// An id for the recovered profile: `wanted`, else the one it had, made unique.
pub fn recovered_id(
    store: &flux_core::store::ProfileStore,
    found: &Found,
    wanted: Option<&str>,
) -> Result<ProfileId, String> {
    let base = match wanted {
        Some(w) => ProfileId::new(w).map_err(|e| e.to_string())?,
        None => {
            let kept = found.kept.profile.split('+').next().unwrap_or("backup");
            ProfileId::new(kept)
                .ok()
                .or_else(|| ProfileId::slugify(&found.name))
                .ok_or("give it an id")?
        }
    };
    if wanted.is_some() {
        return if store.exists(&base) {
            Err(format!("a backup “{base}” exists already"))
        } else {
            Ok(base)
        };
    }
    let mut id = base.clone();
    let mut n = 2;
    while store.exists(&id) {
        id = ProfileId::new(format!(
            "{}-{n}",
            &base.as_str()[..base.as_str().len().min(60)]
        ))
        .map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(id)
}

/// Make the profile for `found`, save it, and when it's the same names as this computer's
/// own (made here, under the same id), drop the origin. Without saved settings, the
/// folders are the newest snapshot's.
pub fn recover(
    paths: &Paths,
    found: &Found,
    target: &Target,
    then: Then,
    id: Option<&str>,
) -> Result<(ProfileId, Profile), String> {
    let store = flux_core::store::ProfileStore::new(paths);
    let id = recovered_id(&store, found, id)?;
    let mut p = recovered_profile(found, target, then);
    if p.sources.paths.is_empty()
        && let Ok(repo) = crate::repo::AnyRepo::open(paths, &id, &p, None)
        && let Some(newest) = repo
            .snapshots()
            .ok()
            .and_then(|s| s.into_iter().max_by_key(|m| m.started))
    {
        p.sources.paths = newest.sources;
    }
    if p.origin.as_ref().is_some_and(|o| {
        o.host == sys::hostname() && o.user == sys::username() && o.profile == id.as_str()
    }) {
        p.origin = None;
    }
    let issues = p.validate();
    if let Some(i) = issues
        .iter()
        .find(|i| i.severity == flux_core::Severity::Error)
    {
        return Err(format!("{}: {}", i.field, i.message));
    }
    store.create(&id, &p).map_err(|e| e.to_string())?;
    Ok((id, p))
}

/// What only you can tell (Flux asks): kept on this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Answers {
    /// The password is kept somewhere other than this computer.
    pub password_elsewhere: bool,
    /// The Borg key was exported.
    pub key_exported: bool,
    /// A recovery code was made for the Vault (and not removed since), by Flux.
    pub recovery_code: bool,
}

fn answers_path(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths
        .state_dir
        .join("recovery")
        .join(format!("{id}.answers.json"))
}

pub fn answers(paths: &Paths, id: &ProfileId) -> Answers {
    std::fs::read(answers_path(paths, id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Record an answer: `key` is `password_elsewhere` or `key_exported`.
pub fn set_answer(paths: &Paths, id: &ProfileId, key: &str, yes: bool) -> Result<(), String> {
    let mut a = answers(paths, id);
    match key {
        "password_elsewhere" => a.password_elsewhere = yes,
        "key_exported" => a.key_exported = yes,
        "recovery_code" => a.recovery_code = yes,
        _ => return Err(format!("nothing to answer about {key:?}")),
    }
    let path = answers_path(paths, id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(&a).map_err(|e| e.to_string())?;
    flux_core::store::write_atomic(&path, &json, 0o600).map_err(|e| e.to_string())
}

/// One line of the recovery readiness checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub key: &'static str,
    pub ok: bool,
    /// Flux can't know: you tick it ([`set_answer`] with `key`).
    pub asked: bool,
    pub title: String,
    pub detail: String,
}

/// Could this backup be recovered on another computer? What's ready and what isn't.
pub fn readiness(paths: &Paths, id: &ProfileId, p: &Profile) -> Vec<Check> {
    let mut out = Vec::new();
    let a = answers(paths, id);
    let kit = matches!(p.engine, Engine::Rsync | Engine::Borg);
    let copy = match p.engine {
        Engine::Vault => "in the Vault, encrypted",
        _ => "next to them",
    };
    out.push(match copy_current(paths, id, p) {
        Some(true) => Check {
            key: "settings",
            ok: true,
            asked: false,
            title: "Its settings are kept with the backups".into(),
            detail: format!(
                "A copy of the folders, exclusions and rules, {copy}: on a new computer, Recover an existing backup finds it.{}",
                if kit {
                    " The recovery kit (FLUX-RECOVERY.txt) says how to use the backups without Flux."
                } else {
                    ""
                }
            ),
        },
        Some(false) => Check {
            key: "settings",
            ok: false,
            asked: false,
            title: "Its settings changed since they were kept with the backups".into(),
            detail: "The copy is brought up to date at the next backup.".into(),
        },
        None => Check {
            key: "settings",
            ok: false,
            asked: false,
            title: "Its settings aren't kept with the backups yet".into(),
            detail: format!("A copy is kept {copy} after the next backup."),
        },
    });
    if let Some(source) = p.password_source() {
        let unlock = p
            .vault
            .as_ref()
            .and_then(|v| v.unlock_file.as_ref())
            .is_some();
        out.push(Check {
            key: "password_elsewhere",
            ok: a.password_elsewhere,
            asked: true,
            title: if unlock {
                "The password and the unlock file are kept somewhere other than this computer"
                    .into()
            } else {
                "The password is kept somewhere other than this computer".into()
            },
            detail: format!(
                "Without {}, nobody can open these backups, you included. {}",
                if unlock {
                    "both"
                } else {
                    "it"
                },
                match source {
                    flux_core::profile::PasswordSource::Keyring => {
                        "It's in this computer's keyring, which goes with the computer: keep it in a password manager or on paper too."
                    }
                    _ => "Keep it in a password manager or on paper too.",
                }
            ),
        });
    }
    if p.engine == Engine::Vault && p.vault.as_ref().is_none_or(|v| v.encryption) {
        out.push(Check {
            key: "recovery_code",
            ok: a.recovery_code,
            asked: false,
            title: if a.recovery_code {
                "There's a recovery code".into()
            } else {
                "No recovery code yet".into()
            },
            detail: "A long code, printed or kept somewhere safe, that opens the backups when the password is lost.".into(),
        });
    }
    if p.engine == Engine::Borg
        && p.borg
            .as_ref()
            .is_none_or(|b| b.encryption != flux_core::profile::BorgEncryption::None)
    {
        out.push(Check {
            key: "key_exported",
            ok: a.key_exported,
            asked: true,
            title: "The Borg key is exported".into(),
            detail: format!(
                "If the repository's key is damaged, the passphrase alone can't open it. Export the key (borg key export {} borg-key.txt) and keep the file with the passphrase.",
                flux_compat::borg::repo_url(id, p, true)
            ),
        });
    }
    let tests = crate::restore_test::load_state(paths, id);
    out.push(Check {
        key: "restore_test",
        ok: tests.last_passed.is_some(),
        asked: false,
        title: "A restore test has passed".into(),
        detail: match tests.last_passed {
            Some(t) => format!(
                "Files were last read back and checked on {}.",
                t.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B %Y")
            ),
            None => "Flux reads a few files back from the backups and checks every byte against what was stored.".into(),
        },
    });
    let offsite = p.offsite || p.destinations.iter().any(|d| d.offsite);
    out.push(Check {
        key: "offsite",
        ok: offsite,
        asked: false,
        title: "A copy is kept somewhere else".into(),
        detail: if offsite {
            "A fire or a theft wouldn't take every copy.".into()
        } else {
            "A fire or a theft takes a computer and the drive next to it together. Add a place that's kept elsewhere: another drive you keep away from home, or a server.".into()
        },
    });
    out
}

/// The program that reads Vaults without Flux, and the note kept next to it.
pub const TOOL: &str = "flux-vault-recover";
pub const TOOL_NOTE: &str = "FLUX-VAULT-RECOVERY.txt";

/// The recovery tool installed with Flux: `FLUX_VAULT_RECOVER`, else next to this
/// program, else in Flux's libexec folder.
pub fn recovery_tool() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FLUX_VAULT_RECOVER") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join(TOOL),
        dir.join("../libexec/flux-backup").join(TOOL),
        PathBuf::from("/usr/libexec/flux-backup").join(TOOL),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn tool_note() -> String {
    format!(
        "This drive holds backups made by Flux Backup in a Vault (the flux-vault folder). With Flux Backup installed, open it and choose Recover an existing backup.

Without Flux, use flux-vault-recover, the small program next to this note. It runs
on any 64-bit Linux, needs nothing installed, and only reads: the Vault is never
changed. In a terminal, in this folder:

  ./flux-vault-recover flux-vault snapshots
      lists the snapshots (an encrypted Vault's password is asked)

  ./flux-vault-recover flux-vault files latest
      lists the files of the newest snapshot (or give a snapshot's id)

  ./flux-vault-recover flux-vault restore latest ~/Restored
      restores everything into ~/Restored, under the files' full paths

  ./flux-vault-recover flux-vault restore latest ~/Restored home/me/Documents
      restores only that folder

  ./flux-vault-recover flux-vault verify
      reads every stored piece back and reports any damage

If the program won't start (\"Permission denied\"), run: chmod +x flux-vault-recover
If the Vault needs an unlock file, add --unlock-file /path/to/the/file.
If the password is lost but you made a recovery code, type the code when the password
is asked for (it doesn't need the unlock file).
The format is documented at https://github.com/neuralpunk/flux-backup (docs/FORMATS.md).

Kept here by Flux Backup {version}.
",
        version = flux_core::VERSION
    )
}

/// Next to a Vault on a drive: keep the recovery tool `tool` and a note on using it
/// (copied again when either is missing or the tool changed). `Ok(false)`: nothing to do
/// (not a Vault on a drive, turned off, or no tool installed).
pub fn keep_tool(p: &Profile, tool: Option<&Path>) -> Result<bool, String> {
    let (Engine::Vault, Target::Local { path, .. }, Some(tool)) = (p.engine, &p.target, tool)
    else {
        return Ok(false);
    };
    if !p.vault.as_ref().is_none_or(|v| v.keep_recovery_tool) {
        return Ok(false);
    }
    let (dest, note) = (path.join(TOOL), path.join(TOOL_NOTE));
    let text = tool_note();
    let tool_len = std::fs::metadata(tool)
        .map_err(|e| format!("{}: {e}", tool.display()))?
        .len();
    let current = std::fs::metadata(&dest).is_ok_and(|m| m.len() == tool_len)
        && std::fs::read_to_string(&note).is_ok_and(|t| t == text)
        && std::fs::read(&dest).ok() == std::fs::read(tool).ok();
    if current {
        return Ok(false);
    }
    let bytes = std::fs::read(tool).map_err(|e| format!("{}: {e}", tool.display()))?;
    flux_core::store::write_atomic(&dest, &bytes, 0o755).map_err(|e| e.to_string())?;
    flux_core::store::write_atomic(&note, text.as_bytes(), 0o644).map_err(|e| e.to_string())?;
    Ok(true)
}
