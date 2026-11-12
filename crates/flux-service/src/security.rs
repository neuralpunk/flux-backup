//! A Vault's security settings that live in the Vault itself (its key slots), and the app
//! locking Vaults again after a while.
//!
//! How hard a password is hashed, and whether it needs an unlock file, are properties of
//! the key slot in the Vault, which may be shared by several computers. So they change by
//! an action on the Vault (needing its password), not by saving the profile. The profile
//! keeps where this computer finds the unlock file, and the hashing used for new passwords.

use std::path::PathBuf;

use serde::Serialize;

use flux_core::profile::{Engine, PasswordHashing, Profile};
use flux_core::{ProfileId, ProfileStore};
use flux_runner::password::{self, Purpose};
use flux_runner::repo::RepoError;
use flux_vault::crypto::KdfParams;
use flux_vault::repo::KeyInfo;

use crate::{ApiError, Result, Service};

/// What the Security settings show.
#[derive(Debug, Clone, Serialize)]
pub struct VaultSecurity {
    /// The Vault's passwords (key slots): label, hashing, unlock file. Empty when the Vault
    /// doesn't exist yet.
    pub keys: Vec<KeyInfo>,
    /// Known only once the Vault is unlocked in the app: which key is the one in use.
    pub unlocked: bool,
    /// The hashing the profile uses for new passwords.
    pub hashing: PasswordHashing,
    pub unlock_file: Option<PathBuf>,
    /// The unlock file is where the profile says.
    pub unlock_file_present: bool,
    /// How long unlocking takes on this computer with the profile's hashing.
    pub unlock_ms: u64,
}

fn vault_profile(state: &Service, id: &str) -> Result<(ProfileId, Profile)> {
    let (pid, p) = state.load(id)?;
    if p.engine != Engine::Vault || !p.vault.as_ref().is_none_or(|v| v.encryption) {
        return Err(ApiError::new(
            "error",
            "Only encrypted Vault backups have these settings.",
        ));
    }
    Ok((pid, p))
}

/// How long hashing one password takes here, in milliseconds.
pub fn time_hashing_impl(hashing: PasswordHashing) -> Result<u64> {
    let t = KdfParams::of(hashing)
        .time_it()
        .map_err(|e| ApiError::new("error", e.to_string()))?;
    Ok(t.as_millis() as u64)
}

pub fn vault_security_impl(state: &Service, id: String) -> Result<VaultSecurity> {
    let (pid, p) = vault_profile(state, &id)?;
    let v = p.vault.clone().unwrap_or_default();
    let (keys, unlocked) = match state.password(&pid) {
        Some(_) => match state.open(&pid, &p) {
            Ok(repo) => (
                repo.vault()
                    .map(|r| r.keys())
                    .transpose()?
                    .unwrap_or_default(),
                true,
            ),
            Err(e) if e.code == "empty" => (Vec::new(), true),
            Err(e) => return Err(e),
        },
        // Key slots are plain JSON: they can be listed without the password.
        None => {
            let pool = flux_runner::repo::sftp_pool(&state.paths, &p, true);
            let backend = flux_runner::repo::vault_backend(&p, pool);
            (
                flux_vault::VaultRepo::list_keys(backend.as_ref()).unwrap_or_default(),
                false,
            )
        }
    };
    Ok(VaultSecurity {
        keys,
        unlocked,
        hashing: v.hashing,
        unlock_file_present: v.unlock_file.as_ref().is_some_and(|f| f.is_file()),
        unlock_file: v.unlock_file,
        unlock_ms: time_hashing_impl(v.hashing)?,
    })
}

/// The Vault, opened with the password the app was given (or, for a keyring, a file or a
/// command, from there), and that password.
fn open_with_password(
    state: &Service,
    pid: &ProfileId,
    p: &Profile,
    given: Option<&password::Secret>,
) -> Result<(flux_vault::VaultRepo, password::Secret)> {
    let cached = given.cloned().or_else(|| state.password(pid));
    let ask = move |_: &str, _: bool| cached.clone();
    let pw = password::obtain(pid, p, Purpose::Open, Some(&ask)).map_err(|_| {
        ApiError::new(
            "password_required",
            "Unlock the backups first: this needs the Vault's password.",
        )
    })?;
    let given = pw.clone();
    let ask = move |_: &str, _: bool| Some(given.clone());
    let repo = flux_runner::repo::open_vault(&state.paths, pid, p, false, Some(&ask))?;
    Ok((repo, pw))
}

/// Hash the password in use again, with `hashing`, and make that the profile's setting for
/// new passwords too. Instant for the data: only the key slot is rewritten.
pub fn strengthen_impl(state: &Service, id: String, hashing: PasswordHashing) -> Result<()> {
    strengthen(state, id, hashing, None)
}

/// [`strengthen_impl`] with a password given here (the command line asks for it).
pub fn strengthen(
    state: &Service,
    id: String,
    hashing: PasswordHashing,
    password: Option<&password::Secret>,
) -> Result<()> {
    let (pid, mut p) = vault_profile(state, &id)?;
    let (repo, pw) = open_with_password(state, &pid, &p, password)?;
    repo.rewrap(&pw, KdfParams::of(hashing), repo.unlock_in_use())?;
    p.vault.get_or_insert_with(Default::default).hashing = hashing;
    ProfileStore::new(&state.paths).save(&pid, &p)?;
    state.forget_cached(&pid);
    Ok(())
}

/// Make a recovery code for the Vault (returned, to show once), or with `remove` remove
/// it. The recovery checklist remembers which. Only for your own backups: a system
/// backup's secrets stay with root.
pub fn recovery_code_impl(state: &Service, id: String, remove: bool) -> Result<Option<String>> {
    if state.scope() == flux_core::Scope::System {
        return Err(ApiError::new(
            "not_here",
            "A system backup's recovery code is made with `sudo flux --system key recovery`.",
        ));
    }
    let (pid, p) = vault_profile(state, &id)?;
    let (repo, _) = open_with_password(state, &pid, &p, None)?;
    let _lock = repo.lock(true)?;
    let code = if remove {
        repo.remove_recovery_code()?;
        None
    } else {
        Some(repo.set_recovery_code()?.to_string())
    };
    flux_runner::recover::set_answer(&state.paths, &pid, "recovery_code", !remove)
        .map_err(|e| ApiError::new("error", e))?;
    Ok(code)
}

/// What replacing a lost password did.
#[derive(Debug, Clone, Serialize)]
pub struct Replaced {
    /// The new recovery code, to show once. `None` for a system backup, whose code root
    /// makes (`sudo flux --system key recovery`); the old one still works until then.
    pub recovery_code: Option<String>,
    /// Where the new password is kept now: "keyring", "file", "ask" (nowhere: asked for
    /// each time), or "command" (the profile's command, which Flux can't change).
    pub kept: &'static str,
    /// The new password needs the unlock file, as the old one did.
    pub unlock_file: bool,
    /// The profile's unlock file wasn't there, so the new password works without it and
    /// the profile no longer names it.
    pub unlock_file_dropped: Option<PathBuf>,
    /// How many old passwords were removed.
    pub removed: usize,
}

/// After the Vault was unlocked with its recovery code (the password was lost): make
/// `password` its password, remove the key slots in `remove` (the lost passwords, which
/// would otherwise still open it), and make a new recovery code in place of the one just
/// used, which may now be in the clipboard or seen. Then keep the new password where the
/// profile keeps its password.
///
/// No backup data is re-encrypted: the passwords and the code each wrap the same data
/// key, which opening with the code never revealed to anyone but this computer. Removing
/// a slot is what stops a password working.
pub fn replace_password_impl(
    state: &Service,
    id: String,
    password: String,
    remove: Vec<String>,
) -> Result<Replaced> {
    use flux_core::profile::PasswordSource;
    let password = zeroize::Zeroizing::new(password);
    if password.is_empty() {
        return Err(ApiError::new("error", "Choose a new password."));
    }
    let (pid, mut p) = vault_profile(state, &id)?;
    // Opened with what the app was given (the code), not where the profile keeps its
    // password: that may still hold the lost one.
    let given = state.password(&pid).ok_or_else(|| {
        ApiError::new(
            "password_required",
            "Unlock the backups with the recovery code first.",
        )
    })?;
    let mut typed = p.clone();
    typed
        .vault
        .get_or_insert_with(Default::default)
        .password_source = PasswordSource::Prompt;
    let ask = move |_: &str, _: bool| Some(given.clone());
    let repo = flux_runner::repo::open_vault(&state.paths, &pid, &typed, false, Some(&ask))?;
    if !repo.opened_by_recovery_code()? {
        return Err(ApiError::new(
            "error",
            "This is for after unlocking with the recovery code. To change a password you know, use `flux key passwd`.",
        ));
    }
    let keys = repo.keys()?;
    for r in &remove {
        if !keys
            .iter()
            .any(|k| &k.id == r && k.label != flux_vault::crypto::RECOVERY_LABEL)
        {
            return Err(ApiError::new(
                "error",
                format!("This Vault has no password {r} to remove."),
            ));
        }
    }
    let _lock = repo.lock(true)?;
    // The new password needs the unlock file when the profile has one and it's here (the
    // code opened the Vault without it). When it's gone, the new password can't need it.
    let unlock = flux_runner::repo::unlock_secret(&p)?;
    let dropped = match (&unlock, p.vault.as_mut()) {
        (None, Some(v)) => v.unlock_file.take(),
        _ => None,
    };
    if dropped.is_some() {
        ProfileStore::new(&state.paths).save(&pid, &p)?;
    }
    // New keys first, then the old ones go: at no moment does nothing open the Vault.
    repo.add_key_with(&password, "password", unlock.as_ref())?;
    let system = state.scope() == flux_core::Scope::System;
    let code = if system {
        None
    } else {
        Some(repo.set_recovery_code()?.to_string())
    };
    for r in &remove {
        repo.remove_key(r)?;
    }
    let kept = match p.password_source() {
        Some(PasswordSource::Keyring) if !system => {
            password::keyring_store(&pid, &p.name, &password).map_err(RepoError::from)?;
            "keyring"
        }
        Some(PasswordSource::File { path }) => {
            replace_secret_file(&path, &password)?;
            "file"
        }
        Some(PasswordSource::Command { .. }) => "command",
        _ => "ask",
    };
    if code.is_some() {
        flux_runner::recover::set_answer(&state.paths, &pid, "recovery_code", true)
            .map_err(|e| ApiError::new("error", e))?;
    }
    state.forget_cached(&pid);
    state.remember_password(&pid, password);
    Ok(Replaced {
        recovery_code: code,
        kept,
        unlock_file: unlock.is_some(),
        unlock_file_dropped: dropped,
        removed: remove.len(),
    })
}

/// Put `secret` in the file at `path` in place of what it held, readable by its owner
/// only (written beside it, then moved over it).
fn replace_secret_file(path: &std::path::Path, secret: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map_or_else(Default::default, |n| n.to_string_lossy());
    let tmp = dir.join(format!(".{name}.flux-new"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(secret.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What turning the unlock file on or off did.
#[derive(Debug, Clone, Serialize)]
pub struct UnlockFileResult {
    pub unlock_file: Option<PathBuf>,
    /// Other passwords of the Vault that still open it without the file.
    pub without_file: Vec<KeyInfo>,
}

/// Require an unlock file for the password in use (`path`: a new file Flux creates there
/// when `create`, else an existing file), or stop requiring one (`path: None`).
pub fn set_unlock_file_impl(
    state: &Service,
    id: String,
    path: Option<String>,
    create: bool,
) -> Result<UnlockFileResult> {
    set_unlock_file(state, id, path, create, None)
}

/// [`set_unlock_file_impl`] with a password given here (the command line asks for it).
pub fn set_unlock_file(
    state: &Service,
    id: String,
    path: Option<String>,
    create: bool,
    password: Option<&password::Secret>,
) -> Result<UnlockFileResult> {
    let (pid, mut p) = vault_profile(state, &id)?;
    let (repo, pw) = open_with_password(state, &pid, &p, password)?;
    let current = repo
        .keys()?
        .into_iter()
        .find(|k| k.current)
        .ok_or_else(|| ApiError::new("error", "This Vault isn't encrypted."))?;
    let kdf = KdfParams::of(current.hashing);
    let store = ProfileStore::new(&state.paths);
    match path {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(ApiError::new("error", "Choose where the unlock file goes."));
            }
            let secret = if create {
                flux_runner::repo::create_unlock_file(&path)?
            } else {
                p.vault.get_or_insert_with(Default::default).unlock_file = Some(path.clone());
                flux_runner::repo::unlock_secret(&p)?
                    .ok_or_else(|| RepoError::UnlockFileMissing(path.clone()))?
            };
            // The profile learns where the file is before the Vault starts needing it: if
            // anything fails in between, the file is merely not needed yet.
            p.vault.get_or_insert_with(Default::default).unlock_file = Some(path.clone());
            store.save(&pid, &p)?;
            repo.rewrap(&pw, kdf, Some(&secret))?;
        }
        None => {
            repo.rewrap(&pw, kdf, None)?;
            p.vault.get_or_insert_with(Default::default).unlock_file = None;
            store.save(&pid, &p)?;
        }
    }
    state.forget_cached(&pid);
    let (repo, _) = open_with_password(state, &pid, &p, Some(&pw))?;
    let without_file = repo
        .keys()?
        .into_iter()
        .filter(|k| !k.current && !k.unlock_file)
        .collect();
    Ok(UnlockFileResult {
        unlock_file: p.vault.and_then(|v| v.unlock_file),
        without_file,
    })
}

/// Forget the password the app was given for this backup (lock it again now).
pub fn lock_impl(state: &Service, id: String) -> Result<()> {
    let (pid, _) = state.load(&id)?;
    state.forget_password(&pid);
    state.forget_cached(&pid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::{PasswordSource, Target, VaultOptions};
    use flux_core::{Paths, Scope};

    const FAST: PasswordHashing = PasswordHashing::Custom {
        memory_mib: 8,
        passes: 1,
        threads: 1,
    };
    const FASTER: PasswordHashing = PasswordHashing::Custom {
        memory_mib: 9,
        passes: 1,
        threads: 2,
    };

    fn setup(tmp: &std::path::Path) -> (Service, ProfileId, Profile) {
        std::fs::create_dir_all(tmp.join("tgt")).unwrap();
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        let paths = Paths::under(tmp.join("home"), Scope::User);
        let mut p = Profile::new(
            "Test",
            Target::Local {
                path: tmp.join("tgt"),
                fs_uuid: None,
            },
            vec![tmp.join("src")],
        );
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions {
            password_source: PasswordSource::Prompt,
            hashing: FAST,
            ..VaultOptions::default()
        });
        let id = ProfileId::new("t").unwrap();
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        (Service::new(paths), id, p)
    }

    #[test]
    fn strengthen_and_unlock_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, id, _) = setup(tmp.path());
        // Locked and no Vault yet: nothing to list, nothing to do.
        let s = vault_security_impl(&state, "t".into()).unwrap();
        assert!(s.keys.is_empty() && !s.unlocked && s.hashing == FAST);
        assert_eq!(
            strengthen_impl(&state, "t".into(), FASTER)
                .unwrap_err()
                .code(),
            "password_required"
        );

        crate::views::set_password_impl(&state, "t".into(), "pw".into()).unwrap();
        crate::views::unlock_impl(&state, "t".into(), "pw".into(), false).unwrap();
        let s = vault_security_impl(&state, "t".into()).unwrap();
        assert!(s.unlocked);
        assert_eq!(s.keys.len(), 1);
        assert!(s.keys[0].current && s.keys[0].hashing == FAST && !s.keys[0].unlock_file);

        // Stronger hashing: the slot is rewritten, the profile remembers it.
        strengthen_impl(&state, "t".into(), FASTER).unwrap();
        let s = vault_security_impl(&state, "t".into()).unwrap();
        assert_eq!(s.keys.len(), 1);
        assert_eq!(s.keys[0].hashing, FASTER);
        assert_eq!(s.hashing, FASTER);

        // A spare password, then an unlock file for the one in use.
        let (_, p) = state.load("t").unwrap();
        let (repo, _) = open_with_password(&state, &id, &p, None).unwrap();
        repo.add_key("spare", "spare").unwrap();
        let file = tmp.path().join("stick/flux.unlock");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let r = set_unlock_file_impl(&state, "t".into(), Some(file.display().to_string()), true)
            .unwrap();
        assert_eq!(r.unlock_file.as_deref(), Some(file.as_path()));
        assert_eq!(r.without_file.len(), 1, "the spare still opens it alone");
        assert_eq!(std::fs::read(&file).unwrap().len(), 64);
        let s = vault_security_impl(&state, "t".into()).unwrap();
        assert!(s.unlock_file_present);
        assert!(s.keys.iter().any(|k| k.current && k.unlock_file));
        // Creating never overwrites a file.
        assert!(
            set_unlock_file_impl(&state, "t".into(), Some(file.display().to_string()), true)
                .is_err()
        );

        // Without the file, the password alone no longer opens it, and the app says why.
        lock_impl(&state, "t".into()).unwrap();
        let moved = tmp.path().join("elsewhere");
        std::fs::rename(&file, &moved).unwrap();
        let e = crate::views::unlock_impl(&state, "t".into(), "pw".into(), false);
        assert!(e.is_err());
        let e = {
            crate::views::unlock_impl(&state, "t".into(), "pw".into(), false).ok();
            let (_, p) = state.load("t").unwrap();
            let given = zeroize::Zeroizing::new("pw".to_string());
            let ask = move |_: &str, _: bool| Some(given.clone());
            flux_runner::repo::open_vault(&state.paths, &id, &p, false, Some(&ask)).unwrap_err()
        };
        let api: ApiError = e.into();
        assert_eq!(api.code(), "unlock_file", "{api}");
        assert!(api.to_string().contains("flux.unlock"), "{api}");

        // With it back: opens; turning it off makes the password enough again.
        std::fs::rename(&moved, &file).unwrap();
        crate::views::unlock_impl(&state, "t".into(), "pw".into(), false).unwrap();
        let r = set_unlock_file_impl(&state, "t".into(), None, false).unwrap();
        assert!(r.unlock_file.is_none());
        std::fs::remove_file(&file).unwrap();
        lock_impl(&state, "t".into()).unwrap();
        crate::views::unlock_impl(&state, "t".into(), "pw".into(), false).unwrap();
        let s = vault_security_impl(&state, "t".into()).unwrap();
        assert!(s.keys.iter().all(|k| !k.unlock_file));
    }

    #[test]
    fn a_lost_password_replaced_after_the_recovery_code() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, id, _) = setup(tmp.path());
        let unlock = |pw: &str| crate::views::unlock_impl(&state, "t".into(), pw.into(), false);
        crate::views::set_password_impl(&state, "t".into(), "lost".into()).unwrap();
        assert!(!unlock("lost").unwrap().recovery_code);
        let old_code = recovery_code_impl(&state, "t".into(), false)
            .unwrap()
            .unwrap();
        let (_, p) = state.load("t").unwrap();
        let (repo, _) = open_with_password(&state, &id, &p, None).unwrap();
        repo.add_key("spare", "spare").unwrap();
        // The password needs an unlock file; the code doesn't.
        let file = tmp.path().join("stick/flux.unlock");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        set_unlock_file_impl(&state, "t".into(), Some(file.display().to_string()), true).unwrap();
        // Not opened with the code: refused.
        assert!(replace_password_impl(&state, "t".into(), "new".into(), vec![]).is_err());

        // Lost: opened with the code, typed loosely.
        lock_impl(&state, "t".into()).unwrap();
        assert!(
            unlock(&old_code.to_lowercase().replace('-', " "))
                .unwrap()
                .recovery_code
        );
        let keys = vault_security_impl(&state, "t".into()).unwrap().keys;
        let lost = keys
            .iter()
            .find(|k| k.label == "initial")
            .unwrap()
            .id
            .clone();
        let code_slot = keys.iter().find(|k| k.current).unwrap().id.clone();
        assert!(replace_password_impl(&state, "t".into(), String::new(), vec![]).is_err());
        assert!(
            replace_password_impl(&state, "t".into(), "new".into(), vec![code_slot]).is_err(),
            "the code isn't a password to remove"
        );
        let r =
            replace_password_impl(&state, "t".into(), "new".into(), vec![lost.clone()]).unwrap();
        assert_eq!(r.kept, "ask");
        assert!(r.unlock_file && r.unlock_file_dropped.is_none());
        assert_eq!(r.removed, 1);
        let new_code = r.recovery_code.unwrap();
        assert_ne!(new_code, old_code);
        // Still unlocked, with the new password.
        assert!(vault_security_impl(&state, "t".into()).unwrap().unlocked);

        let keys = vault_security_impl(&state, "t".into()).unwrap().keys;
        assert_eq!(keys.len(), 3, "{keys:?}"); // spare, new, new code
        assert!(keys.iter().any(|k| k.label == "password" && k.unlock_file));
        assert!(!keys.iter().any(|k| k.id == lost));
        for (pw, opens) in [
            ("lost", false),
            (old_code.as_str(), false),
            ("new", true),
            ("spare", true),
            (new_code.as_str(), true),
        ] {
            lock_impl(&state, "t".into()).unwrap();
            assert_eq!(unlock(pw).is_ok(), opens, "{pw}");
        }
        // Without the file the new password doesn't open it, as the old one didn't.
        std::fs::remove_file(&file).unwrap();
        lock_impl(&state, "t".into()).unwrap();
        assert!(unlock("new").is_err());

        // The file is gone too: the next new password works without it, and the profile
        // stops naming it.
        assert!(unlock(&new_code).unwrap().recovery_code);
        let r = replace_password_impl(&state, "t".into(), "newer".into(), vec![]).unwrap();
        assert!(!r.unlock_file);
        assert_eq!(r.unlock_file_dropped.as_deref(), Some(file.as_path()));
        let (_, p) = state.load("t").unwrap();
        assert!(p.vault.unwrap().unlock_file.is_none());
        lock_impl(&state, "t".into()).unwrap();
        assert!(unlock("newer").is_ok());
    }

    #[test]
    fn a_replaced_password_kept_where_the_old_one_was() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, id, mut p) = setup(tmp.path());
        let key = tmp.path().join("keys/t");
        crate::views::set_password_impl(&state, "t".into(), "lost".into()).unwrap();
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "lost\n").unwrap();
        p.vault.as_mut().unwrap().password_source = PasswordSource::File { path: key.clone() };
        ProfileStore::new(&state.paths).save(&id, &p).unwrap();
        let code = recovery_code_impl(&state, "t".into(), false)
            .unwrap()
            .unwrap();
        lock_impl(&state, "t".into()).unwrap();
        crate::views::unlock_impl(&state, "t".into(), code, false).unwrap();
        let r = replace_password_impl(&state, "t".into(), "new".into(), vec![]).unwrap();
        assert_eq!(r.kept, "file");
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "new\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn timing_and_other_engines() {
        assert!(time_hashing_impl(FAST).unwrap() < 5_000);
        let tmp = tempfile::tempdir().unwrap();
        let (state, id, mut p) = setup(tmp.path());
        p.engine = Engine::Mirror;
        p.vault = None;
        ProfileStore::new(&state.paths).save(&id, &p).unwrap();
        assert!(vault_security_impl(&state, "t".into()).is_err());
    }
}
