//! What the desktop app shows and does, independent of the window: profile views, editing,
//! snapshot actions and restores. The GUI uses it in-process for your own profiles, and
//! `flux-helper` uses the very same code as root for system profiles, behind a small
//! typed protocol ([`protocol`]).
//!
//! Everything goes through `flux-runner`, so both behave exactly like the CLI.

pub mod actions;
pub mod client;
pub mod edit;
pub mod extras;
pub mod filediff;
pub mod find;
pub mod prefs;
pub mod protocol;
pub mod space;
pub mod views;
pub mod watch;

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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
    serde_json::Error,
    VaultError
);

pub type Result<T> = std::result::Result<T, ApiError>;

/// Snapshot entries cached for browsing (a few recent snapshots).
const CACHE_SNAPSHOTS: usize = 4;

/// (profile id, snapshot id) → that snapshot's entries and folder index, most recently
/// used first.
type EntryCache = VecDeque<((String, String), Arc<views::SnapshotIndex>)>;

/// One scope's profiles (yours, or the system's) and what the app remembers about them.
pub struct Service {
    pub paths: Paths,
    /// Vault passwords typed into the app, in memory only.
    passwords: Mutex<HashMap<String, Zeroizing<String>>>,
    entries: Mutex<EntryCache>,
    /// How to start a background backup: program and leading arguments, then the id.
    backup_cmd: Vec<OsString>,
    /// The `flux` CLI that timers run.
    unit_bin: PathBuf,
    /// The latest size estimate; older ones still running stop.
    pub(crate) estimate_gen: std::sync::atomic::AtomicU64,
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
            backup_cmd,
            unit_bin,
            estimate_gen: std::sync::atomic::AtomicU64::new(0),
        }
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
        self.entries
            .lock()
            .expect("cache")
            .retain(|((p, _), _)| p != id.as_str());
    }

    pub fn open(&self, id: &ProfileId, p: &Profile) -> Result<AnyRepo> {
        let cached = self.password(id);
        // Never prompt from the backend: either we know the password, or the frontend asks.
        let ask = move |_: &str, _: bool| cached.clone();
        Ok(AnyRepo::open(&self.paths, id, p, Some(&ask))?)
    }

    fn password(&self, id: &ProfileId) -> Option<Zeroizing<String>> {
        self.passwords
            .lock()
            .expect("passwords")
            .get(id.as_str())
            .cloned()
    }

    fn remember_password(&self, id: &ProfileId, password: Zeroizing<String>) {
        self.passwords
            .lock()
            .expect("passwords")
            .insert(id.to_string(), password);
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
pub fn flux_cli_path() -> PathBuf {
    if let Some(b) = std::env::var_os("FLUX_BIN") {
        return PathBuf::from(b);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join("flux").is_file()
    {
        return dir.join("flux");
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("flux"))
        .find(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/flux"))
}
