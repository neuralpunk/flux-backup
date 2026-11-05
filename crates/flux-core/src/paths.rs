//! Where Flux keeps its files, following the XDG Base Directory spec for user profiles and
//! FHS locations for system (root-mode) profiles.

use std::env;
use std::path::{Path, PathBuf};

use serde::Serialize;

const APP_DIR: &str = "flux-backup";

/// Whether we operate on the invoking user's profiles or on system-wide (root) profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    User,
    System,
}

/// Resolved directory layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Paths {
    pub scope: Scope,
    /// Global settings (`config.toml`) and the `profiles/` directory.
    pub config_dir: PathBuf,
    /// Persistent state: run history, logs, last-run markers.
    pub state_dir: PathBuf,
    /// Disposable caches (e.g. Vault index caches).
    pub cache_dir: PathBuf,
    /// Volatile runtime files: job locks, status sockets.
    pub runtime_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("cannot determine home directory: $HOME is not set")]
    NoHome,
}

impl Paths {
    /// Resolve paths for `scope` from the environment.
    ///
    /// `FLUX_HOME`, when set, puts everything under one directory (handy for tests and
    /// portable setups) and takes precedence over XDG variables.
    pub fn resolve(scope: Scope) -> Result<Self, PathsError> {
        if let Some(root) = env::var_os("FLUX_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::under(PathBuf::from(root), scope));
        }
        match scope {
            Scope::System => Ok(Self {
                scope,
                config_dir: PathBuf::from("/etc").join(APP_DIR),
                state_dir: PathBuf::from("/var/lib").join(APP_DIR),
                cache_dir: PathBuf::from("/var/cache").join(APP_DIR),
                runtime_dir: PathBuf::from("/run").join(APP_DIR),
            }),
            Scope::User => {
                let home = env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .ok_or(PathsError::NoHome)?;
                let xdg = |var: &str, fallback: &str| {
                    env::var_os(var)
                        .map(PathBuf::from)
                        .filter(|p| p.is_absolute())
                        .unwrap_or_else(|| home.join(fallback))
                        .join(APP_DIR)
                };
                let state_dir = xdg("XDG_STATE_HOME", ".local/state");
                let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(APP_DIR))
                    .unwrap_or_else(|| state_dir.join("run"));
                Ok(Self {
                    scope,
                    config_dir: xdg("XDG_CONFIG_HOME", ".config"),
                    cache_dir: xdg("XDG_CACHE_HOME", ".cache"),
                    state_dir,
                    runtime_dir,
                })
            }
        }
    }

    /// Everything under a single root directory.
    pub fn under(root: impl AsRef<Path>, scope: Scope) -> Self {
        let root = root.as_ref();
        Self {
            scope,
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
            runtime_dir: root.join("run"),
        }
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.config_dir.join("profiles")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_root_layout() {
        let p = Paths::under("/x", Scope::User);
        assert_eq!(p.profiles_dir(), PathBuf::from("/x/config/profiles"));
        assert_eq!(p.logs_dir(), PathBuf::from("/x/state/logs"));
    }
}
