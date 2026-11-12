//! What the desktop app shows and does, independent of the window: profile views, editing,
//! snapshot actions and restores. The GUI uses it in-process for your own profiles, and
//! `flux-helper` uses the very same code as root for system profiles, behind a small
//! typed protocol ([`protocol`]).
//!
//! Everything goes through `flux-runner`, so both behave exactly like the CLI.

#![deny(clippy::unwrap_used)]

pub mod actions;
pub mod browse;
pub mod client;
pub mod edit;
pub mod extras;
pub mod filediff;
pub mod find;
pub mod folders;
pub mod prefs;
pub mod protocol;
pub mod recover;
pub mod security;
pub mod space;
pub mod views;
pub mod watch;
pub mod weekly;

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use flux_core::sync::Locked;
use flux_core::{Paths, Profile, ProfileId, ProfileStore, Scope};
use flux_runner::password::PasswordError;
use flux_runner::repo::{AnyRepo, RepoError};
use flux_runner::schedule::UnitEnv;
use flux_vault::VaultError;

/// Error sent to the frontend: a machine-readable code plus a human message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<RepoError> for ApiError {
    fn from(e: RepoError) -> Self {
        let code = match &e {
            RepoError::Password(PasswordError::Unavailable(..))
            | RepoError::Vault(VaultError::PasswordRequired) => "password_required",
            RepoError::Vault(VaultError::WrongPassword)
            | RepoError::Compat(flux_compat::CompatError::WrongPassphrase) => "wrong_password",
            RepoError::Compat(flux_compat::CompatError::PassphraseRequired) => "password_required",
            RepoError::Empty(_) => "empty",
            RepoError::UnlockFileMissing(_) | RepoError::Vault(VaultError::UnlockFileNeeded) => {
                "unlock_file"
            }
            RepoError::Compat(flux_compat::CompatError::Tool(_)) => "tool_missing",
            _ => "error",
        };
        ApiError::new(code, e.to_string())
    }
}

macro_rules! from_err {
    ($($t:ty),*) => {$(
        impl From<$t> for ApiError {
            fn from(e: $t) -> Self {
                ApiError::new("error", e.to_string())
            }
        }
    )*};
}
from_err!(
    flux_core::StoreError,
    flux_core::profile::InvalidProfileId,
    std::io::Error,
    serde_json::Error
);

impl From<VaultError> for ApiError {
    fn from(e: VaultError) -> Self {
        RepoError::Vault(e).into()
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// How long after the last look into a snapshot its file list is kept in memory.
const BROWSE_IDLE: std::time::Duration = std::time::Duration::from_secs(600);

/// Snapshot entries cached for browsing (a few recent snapshots).
const CACHE_SNAPSHOTS: usize = 4;

/// (profile id, snapshot id) → that snapshot's entries and folder index, most recently
/// used first.
type EntryCache = VecDeque<((String, String), Arc<views::SnapshotIndex>)>;

/// One scope's profiles (yours, or the system's) and what the app remembers about them.
pub struct Service {
    pub paths: Paths,
    /// Vault passwords typed into the app, in memory only, with when each was last used.
    passwords: Mutex<HashMap<String, (Zeroizing<String>, std::time::Instant)>>,
    entries: Mutex<EntryCache>,
    /// When a snapshot was last browsed (what `entries` holds is let go a while after).
    pub(crate) browsed: Mutex<Option<std::time::Instant>>,
    /// How to start a background backup: program and leading arguments, then the id.
    backup_cmd: Vec<OsString>,
    /// The `flux` CLI that timers run.
    unit_bin: PathBuf,
    /// The latest size estimate; older ones still running stop.
    pub(crate) estimate_gen: std::sync::atomic::AtomicU64,
    /// The snapshot open in the file manager (*Browse in Files*), if any.
    pub(crate) browsing: Mutex<Option<browse::Mounted>>,
}

impl Service {
    /// Your own profiles, with backups run by the `flux` CLI (see [`flux_cli_path`]).
    pub fn new(paths: Paths) -> Self {
        let flux = flux_cli_path();
        let mut backup_cmd = vec![flux.clone().into_os_string()];
        if paths.scope == Scope::System {
            backup_cmd.push("--system".into());
        }
        backup_cmd.push("backup".into());
        Self::with_commands(paths, backup_cmd, flux)
    }

    /// Explicit commands: `backup_cmd` + profile id starts a backup, `unit_bin` goes into
    /// timers.
    pub fn with_commands(paths: Paths, backup_cmd: Vec<OsString>, unit_bin: PathBuf) -> Self {
        Self {
            paths,
            passwords: Mutex::new(HashMap::new()),
            entries: Mutex::new(VecDeque::new()),
            browsed: Mutex::new(None),
            backup_cmd,
            unit_bin,
            estimate_gen: std::sync::atomic::AtomicU64::new(0),
            browsing: Mutex::new(None),
        }
    }

    /// The `flux` program this service runs (backups' first word).
    pub(crate) fn flux_program(&self) -> &std::ffi::OsStr {
        self.backup_cmd
            .first()
            .map_or(self.unit_bin.as_os_str(), |p| p.as_os_str())
    }

    /// Unmount what *Browse in Files* opened (when the app quits).
    pub fn stop_browsing(&self) {
        let _ = browse::stop_browsing_impl(self);
    }

    pub fn scope(&self) -> Scope {
        self.paths.scope
    }

    pub fn load(&self, id: &str) -> Result<(ProfileId, Profile)> {
        let id = ProfileId::new(id)?;
        let p = ProfileStore::new(&self.paths).load(&id)?;
        Ok((id, p))
    }

    /// Drop cached listings of a profile (after it was edited or its snapshots changed).
    pub fn forget_cached(&self, id: &ProfileId) {
        self.entries.locked().retain(|((p, _), _)| p != id.as_str());
    }

    pub fn open(&self, id: &ProfileId, p: &Profile) -> Result<AnyRepo> {
        let cached = self.password(id);
        // Never prompt from the backend: either we know the password, or the frontend asks.
        let ask = move |_: &str, _: bool| cached.clone();
        Ok(AnyRepo::open(&self.paths, id, p, Some(&ask))?)
    }

    /// The password the app was given for this backup (counts as using it).
    pub(crate) fn password(&self, id: &ProfileId) -> Option<Zeroizing<String>> {
        self.passwords
            .locked()
            .get_mut(id.as_str())
            .map(|(pw, used)| {
                *used = std::time::Instant::now();
                pw.clone()
            })
    }

    pub(crate) fn remember_password(&self, id: &ProfileId, password: Zeroizing<String>) {
        self.passwords
            .locked()
            .insert(id.to_string(), (password, std::time::Instant::now()));
    }

    pub(crate) fn forget_password(&self, id: &ProfileId) {
        self.passwords.locked().remove(id.as_str());
    }

    /// Forget the passwords of backups unused for longer than their "lock again after"
    /// setting. Returns the backups locked again. The app calls this every half minute.
    pub fn expire_idle(&self) -> Vec<String> {
        self.expire_idle_at(std::time::Instant::now())
    }

    fn expire_idle_at(&self, now: std::time::Instant) -> Vec<String> {
        // The snapshots kept for browsing are whole file lists, hundreds of megabytes
        // for a big backup: not held for weeks by an app sitting in the tray.
        let stale = self
            .browsed
            .locked()
            .is_some_and(|at| now.saturating_duration_since(at) >= BROWSE_IDLE);
        if stale {
            self.entries.locked().clear();
            *self.browsed.locked() = None;
        }
        let ids: Vec<(String, std::time::Instant)> = self
            .passwords
            .locked()
            .iter()
            .map(|(id, (_, used))| (id.clone(), *used))
            .collect();
        let mut locked = Vec::new();
        for (id, used) in ids {
            let limit = self
                .load(&id)
                .ok()
                .and_then(|(_, p)| p.vault.and_then(|v| v.lock_after_minutes));
            if let Some(min) = limit
                && now.saturating_duration_since(used)
                    >= std::time::Duration::from_secs(u64::from(min) * 60)
            {
                let pid = ProfileId::new(&id).expect("ids in the map are valid");
                self.forget_password(&pid);
                self.forget_cached(&pid);
                locked.push(id);
            }
        }
        locked
    }

    /// What timers of this scope run.
    pub fn unit_env(&self) -> UnitEnv {
        UnitEnv {
            flux_bin: self.unit_bin.clone(),
            scope: self.paths.scope,
            flux_home: std::env::var_os("FLUX_HOME").map(PathBuf::from),
        }
    }
}

/// The `flux` CLI: `$FLUX_BIN`, else next to this executable, else found on `PATH`, else
/// `/usr/bin/flux`. Always absolute when it can be, since timers need a full path.
///
/// From an AppImage, "next to this executable" is inside the image's mount
/// (`/tmp/.mount_…/usr/bin/flux`), which is gone as soon as the app closes and has
/// another name every time it starts: a timer pointing there would never run, and a
/// backup started from the app would lose its program when the window closes. The
/// bundled `flux` is copied to a lasting place (see [`lasting_cli`]) and that copy is used.
pub fn flux_cli_path() -> PathBuf {
    if let Some(b) = std::env::var_os("FLUX_BIN") {
        return PathBuf::from(b);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join("flux").is_file()
    {
        let beside = dir.join("flux");
        let in_appimage = std::env::var_os("APPIMAGE").is_some()
            && std::env::var_os("APPDIR").is_some_and(|d| exe.starts_with(d));
        if in_appimage && let Some(bin) = cli_home() {
            match lasting_cli(&beside, &bin) {
                Ok(p) => return p,
                Err(e) => tracing::warn!(
                    "couldn't keep a copy of flux at {} ({e}); scheduled backups set up now only run while the app is open",
                    bin.display()
                ),
            }
        }
        return beside;
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("flux"))
        .find(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/flux"))
}

/// Where the AppImage's `flux` is kept for timers: `~/.local/share/flux-backup/bin`.
fn cli_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| flux_core::sys::home_dir().map(|h| h.join(".local/share")))
        .map(|d| d.join("flux-backup/bin"))
}

/// A copy of `bundled` at `dir/flux` that stays when the app closes: made, or replaced
/// when it differs (a newer AppImage). Replaced by renaming, so a backup running the old
/// copy goes on undisturbed.
pub fn lasting_cli(bundled: &std::path::Path, dir: &std::path::Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dest = dir.join("flux");
    let new = std::fs::read(bundled)?;
    if std::fs::read(&dest).is_ok_and(|old| old == new) {
        return Ok(dest);
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(dir)?;
    let tmp = dir.join(format!(".flux.{}.tmp", std::process::id()));
    let written = (|| {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&tmp)?;
        f.write_all(&new)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &dest)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(dest)
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    /// The AppImage's `flux` is copied to where it stays, kept when unchanged, and
    /// replaced by a newer one.
    #[test]
    fn the_bundled_cli_is_kept_where_it_lasts() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let t = tempfile::tempdir().unwrap();
        let bundled = t.path().join("mount/usr/bin/flux");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, "version one").unwrap();
        let dir = t.path().join("share/flux-backup/bin");

        let kept = lasting_cli(&bundled, &dir).unwrap();
        assert_eq!(kept, dir.join("flux"));
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "version one");
        let m = std::fs::metadata(&kept).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o755, "it can be run");

        // Unchanged: the same file stays (a running backup's program isn't swapped).
        lasting_cli(&bundled, &dir).unwrap();
        assert_eq!(std::fs::metadata(&kept).unwrap().ino(), m.ino());
        // A newer AppImage: replaced.
        std::fs::write(&bundled, "version two").unwrap();
        lasting_cli(&bundled, &dir).unwrap();
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "version two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no leftovers");
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn idle_vaults_lock_again() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Service::new(Paths::under(tmp.path().join("home"), Scope::User));
        let store = ProfileStore::new(&state.paths);
        let target = flux_core::profile::Target::Local {
            path: tmp.path().join("t"),
            fs_uuid: None,
        };
        let mut quick = Profile::new("Quick", target.clone(), vec![tmp.path().join("s")]);
        quick.engine = flux_core::Engine::Vault;
        quick.vault = Some(flux_core::profile::VaultOptions {
            lock_after_minutes: Some(5),
            ..Default::default()
        });
        let mut keep = quick.clone();
        keep.vault.as_mut().unwrap().lock_after_minutes = None;
        let (q, k) = (
            ProfileId::new("quick").unwrap(),
            ProfileId::new("keep").unwrap(),
        );
        store.create(&q, &quick).unwrap();
        store.create(&k, &keep).unwrap();
        state.remember_password(&q, Zeroizing::new("a".into()));
        state.remember_password(&k, Zeroizing::new("b".into()));

        let now = std::time::Instant::now();
        assert!(
            state
                .expire_idle_at(now + std::time::Duration::from_secs(4 * 60))
                .is_empty()
        );
        let locked = state.expire_idle_at(now + std::time::Duration::from_secs(6 * 60));
        assert_eq!(locked, vec!["quick".to_string()]);
        assert!(state.password(&q).is_none());
        assert!(state.password(&k).is_some(), "no limit: stays unlocked");
        // Using a password restarts its clock.
        state.remember_password(&q, Zeroizing::new("a".into()));
        let later = std::time::Instant::now() + std::time::Duration::from_secs(4 * 60);
        assert!(state.password(&q).is_some());
        assert!(state.expire_idle_at(later).is_empty());
    }
}
