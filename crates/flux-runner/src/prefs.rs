//! App-wide preferences (`settings.toml` in the config folder) and the pause of
//! scheduled backups (`paused.json` in the state folder). Both are per scope: yours, or
//! the system's.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::Paths;
use flux_core::store::{StoreError, write_atomic};

/// Preferences that aren't about one backup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Also notify when a scheduled backup succeeds (failures and problems always are,
    /// for profiles with notifications on).
    pub notify_success: bool,
    /// The exclusions new profiles start with; `None`: Flux's own suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_exclude: Option<Vec<String>>,
}

impl Settings {
    fn file(paths: &Paths) -> std::path::PathBuf {
        paths.config_dir.join("settings.toml")
    }

    /// The saved settings, or the defaults (a damaged file is logged and ignored).
    pub fn load(paths: &Paths) -> Self {
        match std::fs::read_to_string(Self::file(paths)) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("ignoring settings.toml: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<(), StoreError> {
        let text = toml::to_string_pretty(self).expect("settings serialize");
        std::fs::create_dir_all(&paths.config_dir).map_err(|e| StoreError::Io {
            path: paths.config_dir.clone(),
            source: e,
        })?;
        write_atomic(&Self::file(paths), text.as_bytes(), 0o644)
    }
}

/// Scheduled backups are paused: until a moment, or until resumed (`until: None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub until: Option<Timestamp>,
}

fn pause_file(paths: &Paths) -> std::path::PathBuf {
    paths.state_dir.join("paused.json")
}

/// The pause in effect at `now`, if any. An expired pause is removed.
pub fn pause(paths: &Paths, now: Timestamp) -> Option<Pause> {
    let text = std::fs::read_to_string(pause_file(paths)).ok()?;
    let p: Pause = serde_json::from_str(&text).ok()?;
    match p.until {
        Some(t) if t <= now => {
            let _ = std::fs::remove_file(pause_file(paths));
            None
        }
        _ => Some(p),
    }
}

/// Tomorrow at 7:00 (local time): "until tomorrow" skips tonight's backups and lets the
/// next day's run.
pub fn tomorrow_morning(now: Timestamp) -> Timestamp {
    let tz = jiff::tz::TimeZone::system();
    let z = now.to_zoned(tz.clone());
    let d = z.date().tomorrow().unwrap_or(z.date());
    d.at(7, 0, 0, 0)
        .to_zoned(tz)
        .map(|z| z.timestamp())
        .unwrap_or(now + jiff::SignedDuration::from_hours(24))
}

/// Pause scheduled backups until `until` (or until resumed).
pub fn set_pause(paths: &Paths, until: Option<Timestamp>) -> Result<(), StoreError> {
    std::fs::create_dir_all(&paths.state_dir).map_err(|e| StoreError::Io {
        path: paths.state_dir.clone(),
        source: e,
    })?;
    let text = serde_json::to_string(&Pause { until }).expect("pause serialize");
    write_atomic(&pause_file(paths), text.as_bytes(), 0o644)
}

/// Let scheduled backups run again.
pub fn resume(paths: &Paths) -> std::io::Result<()> {
    match std::fs::remove_file(pause_file(paths)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_pause() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        assert_eq!(Settings::load(&paths), Settings::default());
        let s = Settings {
            notify_success: true,
            default_exclude: Some(vec!["*.iso".into()]),
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), s);

        let now = Timestamp::now();
        assert!(pause(&paths, now).is_none());
        set_pause(&paths, None).unwrap();
        assert_eq!(pause(&paths, now), Some(Pause { until: None }));
        let soon = now + jiff::SignedDuration::from_hours(1);
        set_pause(&paths, Some(soon)).unwrap();
        assert!(pause(&paths, now).is_some());
        assert!(pause(&paths, soon).is_none(), "expired");
        assert!(!pause_file(&paths).exists(), "and removed");
        set_pause(&paths, None).unwrap();
        resume(&paths).unwrap();
        assert!(pause(&paths, now).is_none());
        resume(&paths).unwrap();

        let t = tomorrow_morning(now);
        assert!(t > now && t < now + jiff::SignedDuration::from_hours(32));
    }
}
