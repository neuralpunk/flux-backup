//! Recovering backups on a new computer, and how ready a backup is for that.

use serde::Serialize;

use flux_core::profile::{Engine, KeptAs, Target};

use flux_runner::recover::{self as rc, Check, Search, Then};

use crate::{ApiError, Result, Service};

fn target(value: serde_json::Value) -> Result<Target> {
    serde_json::from_value(value).map_err(|e| ApiError::new("invalid", format!("target: {e}")))
}

/// The Flux backups on a drive or a server; an encrypted Vault's with its password.
pub fn find_backups_impl(
    state: &Service,
    target_v: serde_json::Value,
    password: Option<String>,
) -> Result<Search> {
    let t = target(target_v)?;
    let pw = password.filter(|p| !p.is_empty());
    rc::find(&state.paths, &t, pw.as_deref(), true).map_err(|e| ApiError::new("error", e))
}

#[derive(Serialize)]
pub struct Recovered {
    pub id: String,
    /// A message for people when something was left to do (the password not kept…).
    pub note: Option<String>,
}

/// Set up the backup `kept` (of `engine`) found at `target` on this computer.
pub fn recover_impl(
    state: &Service,
    target_v: serde_json::Value,
    engine: Engine,
    kept: KeptAs,
    then: Then,
    id: Option<String>,
    password: Option<String>,
) -> Result<Recovered> {
    let t = target(target_v)?;
    let pw = password.filter(|p| !p.is_empty());
    let search =
        rc::find(&state.paths, &t, pw.as_deref(), true).map_err(|e| ApiError::new("error", e))?;
    let found = search
        .found
        .iter()
        .find(|f| f.engine == engine && f.kept == kept)
        .ok_or_else(|| ApiError::new("error", "That backup isn't there any more."))?;
    let (pid, p) = rc::recover(
        &state.paths,
        found,
        &t,
        then,
        id.as_deref().filter(|s| !s.is_empty()),
    )
    .map_err(|e| ApiError::new("error", e))?;
    let mut note = None;
    if let Some(pw) = pw
        && p.password_source().is_some()
        && let Err(e) = crate::views::set_password_impl(state, pid.to_string(), pw)
    {
        note = Some(format!(
            "The backup was set up, but its password couldn't be kept: {e}. It's asked for when needed."
        ));
    }
    if then == Then::CarryOn
        && let Some(msg) = crate::edit::apply_timer(state, &pid, &p)
        && msg.starts_with("Saved, but")
    {
        note = Some(msg);
    }
    Ok(Recovered {
        id: pid.to_string(),
        note,
    })
}

/// Could this backup be recovered on another computer?
pub fn readiness_impl(state: &Service, id: String) -> Result<Vec<Check>> {
    let (pid, p) = state.load(&id)?;
    Ok(rc::readiness(&state.paths, &pid, &p))
}

/// Tick (or untick) something only you can tell: `password_elsewhere`, `key_exported`.
pub fn set_readiness_impl(
    state: &Service,
    id: String,
    key: String,
    yes: bool,
) -> Result<Vec<Check>> {
    let (pid, p) = state.load(&id)?;
    rc::set_answer(&state.paths, &pid, &key, yes).map_err(|e| ApiError::new("invalid", e))?;
    Ok(rc::readiness(&state.paths, &pid, &p))
}

/// The readiness actions hand secrets to the person at the screen and write files where
/// they choose: only for their own backups, never through the administrator helper (a
/// system backup's password and unlock file stay readable by root only).
fn own_backup(state: &Service) -> Result<()> {
    if state.scope() == flux_core::Scope::System {
        return Err(ApiError::new(
            "not_here",
            "A system backup's password and unlock file are kept readable by root only.",
        ));
    }
    Ok(())
}

/// The password (or Borg passphrase) of a backup, to keep somewhere else: the one typed
/// into the app this session, or from where it's kept (keyring, file, command). One that's
/// asked for each time isn't kept anywhere, so there's nothing to show.
pub fn reveal_password_impl(state: &Service, id: String) -> Result<String> {
    own_backup(state)?;
    let (pid, p) = state.load(&id)?;
    let Some(source) = p.password_source() else {
        return Err(ApiError::new(
            "not_encrypted",
            "This backup isn't encrypted.",
        ));
    };
    if let Some(pw) = state.password(&pid) {
        return Ok(pw.to_string());
    }
    if source == flux_core::profile::PasswordSource::Prompt {
        return Err(ApiError::new(
            "asked_each_time",
            "Flux asks for this password each time and doesn't keep it: it's only where you keep it.",
        ));
    }
    let pw = flux_runner::password::obtain(&pid, &p, flux_runner::password::Purpose::Open, None)
        .map_err(|e| ApiError::new("password_unavailable", e.to_string()))?;
    Ok(pw.to_string())
}

/// `dir/name`, or `dir/stem-2.ext`… when that's taken: never over an existing file.
fn free_name(dir: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    (1..)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            })
        })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Export a Borg repository's key into `dir` (`borg key export`), as a new file; the
/// checklist's "The Borg key is exported" is ticked. The key file is itself protected by
/// the passphrase, but keep it with the passphrase, not next to the backups.
pub fn export_borg_key_impl(state: &Service, id: String, dir: String) -> Result<String> {
    own_backup(state)?;
    let (pid, p) = state.load(&id)?;
    if p.engine != Engine::Borg {
        return Err(ApiError::new(
            "invalid",
            "Only a Borg repository has a key to export.",
        ));
    }
    let dir = std::path::PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(ApiError::new(
            "invalid",
            format!("{} isn't a folder", dir.display()),
        ));
    }
    let cached = state.password(&pid);
    let ask = move |_: &str, _: bool| cached.clone();
    let repo = flux_runner::repo::open_borg(&state.paths, &pid, &p, false, Some(&ask))?;
    let to = free_name(&dir, &format!("borg-key-{pid}"), "txt");
    repo.export_key(&to)
        .map_err(flux_runner::repo::RepoError::from)?;
    rc::set_answer(&state.paths, &pid, "key_exported", true)
        .map_err(|e| ApiError::new("error", e))?;
    Ok(to.display().to_string())
}

/// Save a copy of a Vault's unlock file into `dir` (a new file only you can read).
pub fn copy_unlock_file_impl(state: &Service, id: String, dir: String) -> Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    own_backup(state)?;
    let (_, p) = state.load(&id)?;
    let Some(from) = p.vault.as_ref().and_then(|v| v.unlock_file.clone()) else {
        return Err(ApiError::new(
            "invalid",
            "This backup doesn't use an unlock file.",
        ));
    };
    let data = std::fs::read(&from).map_err(|e| {
        ApiError::new(
            "unlock_file_missing",
            format!("Can't read the unlock file {}: {e}", from.display()),
        )
    })?;
    let dir = std::path::PathBuf::from(dir);
    let name = from.file_stem().map_or_else(
        || "flux-unlock".into(),
        |s| s.to_string_lossy().into_owned(),
    );
    let ext = from
        .extension()
        .map_or_else(|| "key".into(), |s| s.to_string_lossy().into_owned());
    let to = free_name(&dir, &name, &ext);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&to)?;
    f.write_all(&data)?;
    f.sync_all()?;
    Ok(to.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::{PasswordSource, Profile, VaultOptions};

    fn encrypted(t: &std::path::Path, source: PasswordSource) -> (Service, Profile) {
        let paths = flux_core::Paths::under(t.join("home"), flux_core::Scope::User);
        let mut p = Profile::new(
            "Home",
            Target::Local {
                path: t.join("tgt"),
                fs_uuid: None,
            },
            vec![t.join("src")],
        );
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions {
            password_source: source,
            ..VaultOptions::default()
        });
        let id = flux_core::ProfileId::new("home").unwrap();
        flux_core::store::ProfileStore::new(&paths)
            .create(&id, &p)
            .unwrap();
        (Service::new(paths), p)
    }

    /// The checklist's actions: the password from where it's kept (never one that's asked
    /// for each time, never a system backup's), and the unlock file copied as a new file.
    #[test]
    fn readiness_actions() {
        let t = tempfile::tempdir().unwrap();
        let key = t.path().join("pw");
        std::fs::write(&key, "hunter2\n").unwrap();
        let (s, mut p) = encrypted(t.path(), PasswordSource::File { path: key });
        assert_eq!(reveal_password_impl(&s, "home".into()).unwrap(), "hunter2");

        p.vault.as_mut().unwrap().password_source = PasswordSource::Prompt;
        let id = flux_core::ProfileId::new("home").unwrap();
        flux_core::store::ProfileStore::new(&s.paths)
            .save(&id, &p)
            .unwrap();
        assert_eq!(
            reveal_password_impl(&s, "home".into()).unwrap_err().code(),
            "asked_each_time"
        );
        // Typed into the app this session: that one.
        s.remember_password(&id, zeroize::Zeroizing::new("typed".into()));
        assert_eq!(reveal_password_impl(&s, "home".into()).unwrap(), "typed");

        // The unlock file: copied, readable by its owner only, never over another file.
        let unlock = t.path().join("stick.key");
        std::fs::write(&unlock, b"\x01\x02secret").unwrap();
        p.vault.as_mut().unwrap().unlock_file = Some(unlock.clone());
        flux_core::store::ProfileStore::new(&s.paths)
            .save(&id, &p)
            .unwrap();
        let out = t.path().join("usb");
        std::fs::create_dir_all(&out).unwrap();
        let first = copy_unlock_file_impl(&s, "home".into(), out.display().to_string()).unwrap();
        let second = copy_unlock_file_impl(&s, "home".into(), out.display().to_string()).unwrap();
        assert_eq!(first, out.join("stick.key").display().to_string());
        assert_eq!(second, out.join("stick-2.key").display().to_string());
        assert_eq!(std::fs::read(&first).unwrap(), b"\x01\x02secret");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            export_borg_key_impl(&s, "home".into(), out.display().to_string()).is_err(),
            "a Vault has no Borg key"
        );

        // A system backup's secrets stay with root.
        let sys = Service::new(flux_core::Paths::under(
            t.path().join("sys"),
            flux_core::Scope::System,
        ));
        for e in [
            reveal_password_impl(&sys, "home".into()).unwrap_err(),
            copy_unlock_file_impl(&sys, "home".into(), out.display().to_string()).unwrap_err(),
        ] {
            assert_eq!(e.code(), "not_here");
        }
    }

    /// Exporting a Borg repository's key (with the real borg, when it's installed).
    #[test]
    fn borg_key_export() {
        let has_borg = std::process::Command::new("borg")
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("borg 1."));
        if !has_borg {
            eprintln!("skipped: no borg 1.x");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let key = t.path().join("pw");
        std::fs::write(&key, "correct horse\n").unwrap();
        std::fs::create_dir_all(t.path().join("tgt")).unwrap();
        let (s, mut p) = encrypted(t.path(), PasswordSource::File { path: key.clone() });
        p.engine = Engine::Borg;
        p.vault = None;
        p.borg = Some(flux_core::profile::BorgOptions {
            password_source: PasswordSource::File { path: key },
            ..Default::default()
        });
        let id = flux_core::ProfileId::new("home").unwrap();
        flux_core::store::ProfileStore::new(&s.paths)
            .save(&id, &p)
            .unwrap();
        crate::views::set_password_impl(&s, "home".into(), String::new()).unwrap();
        let out = t.path().join("safe");
        std::fs::create_dir_all(&out).unwrap();
        let path = export_borg_key_impl(&s, "home".into(), out.display().to_string()).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("BORG_KEY "),
            "{path}"
        );
        let checks = readiness_impl(&s, "home".into()).unwrap();
        assert!(
            checks.iter().any(|c| c.key == "key_exported" && c.ok),
            "ticked"
        );
        let again = export_borg_key_impl(&s, "home".into(), out.display().to_string()).unwrap();
        assert_ne!(again, path, "never over the first");
    }

    #[test]
    fn find_recover_and_readiness() {
        let t = tempfile::tempdir().unwrap();
        let (src, tgt) = (t.path().join("src"), t.path().join("tgt"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&tgt).unwrap();
        std::fs::write(src.join("a"), "a").unwrap();
        let old = flux_core::Paths::under(t.path().join("old"), flux_core::Scope::User);
        let id = flux_core::ProfileId::new("home").unwrap();
        let mut p = Profile::new(
            "Home",
            Target::Local {
                path: tgt.clone(),
                fs_uuid: None,
            },
            vec![src],
        );
        p.options.notify = false;
        p.schedule.trigger = flux_core::profile::Trigger::Manual;
        flux_core::store::ProfileStore::new(&old)
            .create(&id, &p)
            .unwrap();
        let r = flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths: &old,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: flux_runner::Trigger::Manual,
                prune: true,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &flux_core::progress::Progress::new(),
            &flux_core::progress::silent,
        );
        assert_eq!(r.record.status, flux_runner::RunStatus::Completed);
        let old_state = Service::new(old.clone());
        let checks = readiness_impl(&old_state, "home".into()).unwrap();
        assert!(checks.iter().any(|c| c.key == "settings" && c.ok));
        assert!(
            set_readiness_impl(&old_state, "home".into(), "password_elsewhere".into(), true)
                .is_ok()
        );

        let new = Service::new(flux_core::Paths::under(
            t.path().join("new"),
            flux_core::Scope::User,
        ));
        let tv = serde_json::json!({ "kind": "local", "path": tgt });
        let s = find_backups_impl(&new, tv.clone(), None).unwrap();
        assert_eq!(s.found.len(), 1);
        let kept = s.found[0].kept.clone();
        let r = recover_impl(
            &new,
            tv.clone(),
            Engine::Mirror,
            kept.clone(),
            Then::Restore,
            None,
            None,
        )
        .unwrap();
        assert_eq!(r.id, "home");
        assert!(r.note.is_none());
        let (_, np) = new.load("home").unwrap();
        assert_eq!(np.name, "Home");
        // Gone: a clear message.
        let gone = KeptAs {
            profile: "other".into(),
            ..kept
        };
        assert!(recover_impl(&new, tv, Engine::Mirror, gone, Then::Restore, None, None).is_err());
    }
}
