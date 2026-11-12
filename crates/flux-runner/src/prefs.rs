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
    /// Run one backup at a time: a backup that starts while another of this scope runs
    /// waits for it to finish.
    #[serde(skip_serializing_if = "is_default")]
    pub one_at_a_time: bool,
    /// The program that compares two versions of a file (`meld`, `kompare`…, with any
    /// arguments; the two files are added). `None`: the first known one installed; empty:
    /// none, only the app's own comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_tool: Option<String>,
    /// What runs scheduled backups: systemd timers (the default) or cron.
    #[serde(skip_serializing_if = "is_default")]
    pub scheduler: crate::schedule::SchedulerKind,
    /// Webhooks told about every backup (each backup can add its own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhooks: Vec<flux_core::webhook::Webhook>,
    /// The app quits when its window is closed, instead of staying in the system tray.
    #[serde(skip_serializing_if = "is_default")]
    pub quit_on_close: bool,
    /// The app has said once that it stays in the tray when its window is closed.
    #[serde(skip_serializing_if = "is_default")]
    pub told_about_tray: bool,
    /// No weekly "all good" notification (it's on unless turned off).
    #[serde(skip_serializing_if = "is_default")]
    pub weekly_note_off: bool,
    /// Hours when the notifications that only inform (a successful backup, the weekly
    /// note) aren't sent. Failures and warnings always are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<QuietHours>,
}

/// From `from` to `to`, local time, as "HH:MM"; past midnight when `to` is earlier
/// ("22:00" to "08:00").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    pub from: String,
    pub to: String,
}

/// "08:00" (or "8:00") as minutes past midnight.
fn minutes(hhmm: &str) -> Option<u16> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl QuietHours {
    /// Both times are times of day, and not the same.
    pub fn is_valid(&self) -> bool {
        matches!((minutes(&self.from), minutes(&self.to)), (Some(a), Some(b)) if a != b)
    }

    /// Is `hour:minute` (local time) inside them? The start counts, the end doesn't.
    pub fn contains(&self, hour: i8, minute: i8) -> bool {
        let (Some(from), Some(to)) = (minutes(&self.from), minutes(&self.to)) else {
            return false;
        };
        let t = (hour.clamp(0, 23) as u16) * 60 + minute.clamp(0, 59) as u16;
        if from < to {
            from <= t && t < to
        } else {
            t >= from || t < to
        }
    }
}

impl Settings {
    /// Whether it's quiet hours now (local time).
    pub fn quiet_now(&self) -> bool {
        let now = jiff::Zoned::now();
        self.quiet_hours
            .as_ref()
            .is_some_and(|q| q.contains(now.hour(), now.minute()))
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
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
        // Webhook addresses and headers can be secrets (tokens, private topics).
        let mode = if self.webhooks.is_empty() {
            0o644
        } else {
            0o600
        };
        write_atomic(&Self::file(paths), text.as_bytes(), mode)
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
    fn quiet_hours_past_midnight_and_within_a_day() {
        let night = QuietHours {
            from: "22:00".into(),
            to: "08:00".into(),
        };
        assert!(night.is_valid());
        assert!(night.contains(22, 0) && night.contains(23, 59) && night.contains(3, 0));
        assert!(night.contains(7, 59) && !night.contains(8, 0) && !night.contains(12, 0));
        let lunch = QuietHours {
            from: "12:30".into(),
            to: "13:30".into(),
        };
        assert!(lunch.contains(12, 30) && lunch.contains(13, 0) && !lunch.contains(13, 30));
        let same = QuietHours {
            from: "9:00".into(),
            to: "09:00".into(),
        };
        assert!(!same.is_valid());
        assert!(
            !QuietHours {
                from: "25:00".into(),
                to: "08:00".into()
            }
            .is_valid()
        );
    }

    #[test]
    fn settings_and_pause() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        assert_eq!(Settings::load(&paths), Settings::default());
        let s = Settings {
            notify_success: true,
            default_exclude: Some(vec!["*.iso".into()]),
            scheduler: crate::schedule::SchedulerKind::Cron,
            one_at_a_time: true,
            diff_tool: Some("meld --newtab".into()),
            webhooks: vec![flux_core::webhook::Webhook {
                url: "https://ntfy.sh/my-backups".into(),
                preset: flux_core::webhook::Preset::Ntfy,
                events: Vec::new(),
                headers: Vec::new(),
                enabled: true,
            }],
            quit_on_close: true,
            told_about_tray: true,
            weekly_note_off: true,
            quiet_hours: Some(QuietHours {
                from: "22:00".into(),
                to: "07:30".into(),
            }),
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), s);
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Paths| {
            std::fs::metadata(p.config_dir.join("settings.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&paths), 0o600, "webhooks can hold secrets");
        Settings::default().save(&paths).unwrap();
        assert_eq!(mode(&paths), 0o644);
        s.save(&paths).unwrap();
        assert!(
            std::fs::read_to_string(paths.config_dir.join("settings.toml"))
                .unwrap()
                .contains("scheduler = \"cron\"")
        );

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
