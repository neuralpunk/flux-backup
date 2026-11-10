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
