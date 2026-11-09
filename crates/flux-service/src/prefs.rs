//! App preferences and pausing scheduled backups.

use jiff::Timestamp;
use serde::Serialize;

use flux_runner::prefs::{self, Pause, Settings};
use flux_runner::schedule::{self, SchedulerKind};

use crate::{ApiError, Result, Service};

/// The exclusions a new profile starts with unless the preferences say otherwise.
pub const DEFAULT_EXCLUDE: &[&str] = &["~/.cache/", "~/.local/share/Trash/", "node_modules/"];

#[derive(Debug, Serialize)]
pub struct Preferences {
    pub notify_success: bool,
    /// A backup that starts while another runs waits for it.
    pub one_at_a_time: bool,
    /// What new profiles start with (Flux's list unless changed).
    pub default_exclude: Vec<String>,
    /// Whether that list was changed from Flux's.
    pub default_exclude_custom: bool,
    pub pause: Option<Pause>,
    /// What runs scheduled backups.
    pub scheduler: SchedulerKind,
    /// Why cron can't be used here, in words (`None`: it can).
    pub cron_problem: Option<String>,
    /// Whether a cron daemon is running (`None`: can't tell).
    pub cron_running: Option<bool>,
    /// The comparison program chosen: a command, `""` for none, `None` for automatic.
    pub diff_tool: Option<String>,
    /// The one used now (`[command, name]`), and the known ones installed.
    pub diff_tool_used: Option<(String, String)>,
    pub diff_tools: Vec<(String, String)>,
}

pub fn preferences_impl(state: &Service) -> Result<Preferences> {
    let s = Settings::load(&state.paths);
    Ok(Preferences {
        notify_success: s.notify_success,
        one_at_a_time: s.one_at_a_time,
        default_exclude_custom: s.default_exclude.is_some(),
        default_exclude: s
            .default_exclude
            .unwrap_or_else(|| DEFAULT_EXCLUDE.iter().map(|&x| x.to_owned()).collect()),
        pause: prefs::pause(&state.paths, Timestamp::now()),
        scheduler: s.scheduler,
        cron_problem: flux_runner::cron::available(state.scope()).err(),
        cron_running: flux_runner::cron::daemon_running(),
        diff_tool: s.diff_tool.clone(),
        diff_tool_used: crate::filediff::diff_tool(state),
        diff_tools: crate::filediff::installed_tools()
            .into_iter()
            .map(|(c, n)| (c.to_owned(), n.to_owned()))
            .collect(),
    })
}

/// Choose the program that compares file versions: a command, `""` for none, `None` for
/// the first known one installed.
pub fn set_diff_tool_impl(state: &Service, tool: Option<String>) -> Result<Preferences> {
    let mut s = Settings::load(&state.paths);
    s.diff_tool = tool.map(|t| t.trim().to_owned());
    s.save(&state.paths)
        .map_err(|e| ApiError::new("error", e.to_string()))?;
    preferences_impl(state)
}

/// Run scheduled backups with `systemd` or `cron`, moving every schedule over.
pub fn set_scheduler_impl(state: &Service, scheduler: String) -> Result<Preferences> {
    let to: SchedulerKind = scheduler
        .parse()
        .map_err(|e: String| ApiError::new("bad_request", e))?;
    let profiles: Vec<_> = flux_core::ProfileStore::new(&state.paths)
        .list()?
        .into_iter()
        .filter_map(|l| match l.profile {
            Ok(p) if !flux_core::profile::has_errors(&p.validate()) => Some((l.id, p)),
            _ => None,
        })
        .collect();
    schedule::switch(&state.paths, to, &profiles, &state.unit_env())
        .map_err(|e| ApiError::new("error", e.to_string()))?;
    preferences_impl(state)
}

/// Save the preferences; `default_exclude: None` goes back to Flux's list.
pub fn save_preferences_impl(
    state: &Service,
    notify_success: bool,
    default_exclude: Option<Vec<String>>,
    one_at_a_time: Option<bool>,
) -> Result<Preferences> {
    let saved = Settings::load(&state.paths);
    let default_exclude = default_exclude.map(|v| {
        v.into_iter()
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
    });
    Settings {
        notify_success,
        default_exclude,
        one_at_a_time: one_at_a_time.unwrap_or(saved.one_at_a_time),
        // Changed only by moving the schedules (`set_scheduler`).
        ..saved
    }
    .save(&state.paths)
    .map_err(|e| ApiError::new("error", e.to_string()))?;
    preferences_impl(state)
}

/// Pause scheduled backups: `hour`, `tomorrow` (7:00), `resumed` (until resumed), or
/// `off` to resume.
pub fn set_pause_impl(state: &Service, pause: String) -> Result<Option<Pause>> {
    let now = Timestamp::now();
    let until = match pause.as_str() {
        "off" => {
            prefs::resume(&state.paths).map_err(|e| ApiError::new("error", e.to_string()))?;
            return Ok(None);
        }
        "hour" => Some(now + jiff::SignedDuration::from_hours(1)),
        "tomorrow" => Some(prefs::tomorrow_morning(now)),
        "resumed" => None,
        other => {
            return Err(ApiError::new(
                "bad_request",
                format!("unknown pause {other:?}"),
            ));
        }
    };
    prefs::set_pause(&state.paths, until).map_err(|e| ApiError::new("error", e.to_string()))?;
    Ok(prefs::pause(&state.paths, now))
}

/// The exclusions for a new profile of this scope's user.
pub fn new_profile_excludes(state: &Service) -> Vec<String> {
    Settings::load(&state.paths)
        .default_exclude
        .unwrap_or_else(|| DEFAULT_EXCLUDE.iter().map(|&x| x.to_owned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let s = Service::new(flux_core::Paths::under(t.path(), flux_core::Scope::User));
        let p = preferences_impl(&s).unwrap();
        assert!(!p.notify_success && !p.default_exclude_custom && p.pause.is_none());
        assert_eq!(p.default_exclude.len(), DEFAULT_EXCLUDE.len());
        let p = save_preferences_impl(
            &s,
            true,
            Some(vec![" *.iso ".into(), "".into()]),
            Some(true),
        )
        .unwrap();
        assert!(p.notify_success && p.default_exclude_custom && p.one_at_a_time);
        assert_eq!(p.default_exclude, ["*.iso"]);
        assert_eq!(new_profile_excludes(&s), ["*.iso"]);
        let e = crate::edit::new_profile_impl(&s).unwrap();
        assert_eq!(e.profile.sources.exclude, ["*.iso"]);
        let p = save_preferences_impl(&s, true, None, None).unwrap();
        assert!(!p.default_exclude_custom);
        assert!(p.one_at_a_time, "left out: unchanged");
        assert!(
            !save_preferences_impl(&s, true, None, Some(false))
                .unwrap()
                .one_at_a_time
        );

        assert!(
            set_pause_impl(&s, "tomorrow".into())
                .unwrap()
                .unwrap()
                .until
                .is_some()
        );
        assert!(
            set_pause_impl(&s, "resumed".into())
                .unwrap()
                .unwrap()
                .until
                .is_none()
        );
        assert!(preferences_impl(&s).unwrap().pause.is_some());
        assert!(set_pause_impl(&s, "off".into()).unwrap().is_none());
        assert!(preferences_impl(&s).unwrap().pause.is_none());
        assert!(set_pause_impl(&s, "never".into()).is_err());
    }
}
