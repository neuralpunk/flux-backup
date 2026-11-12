//! A calm weekly "all good": once a week, one quiet notification when every backup is
//! protected, with nothing to look at. Checked when a scheduled backup ends (so it comes
//! with the app closed too); never sent when something needs attention (those have their
//! own warnings), and a week after Flux first checks, not at once.

use std::path::PathBuf;

use jiff::{SignedDuration, Timestamp};

use flux_core::{ProfileStore, Scope};
use flux_runner::RunStatus;
use flux_runner::prefs::Settings;
use flux_runner::runlog;

use crate::Service;

const WEEK: SignedDuration = SignedDuration::from_hours(7 * 24);

fn stamp(state: &Service) -> PathBuf {
    state.paths.state_dir.join("weekly-note")
}

fn last_sent(state: &Service) -> Option<Timestamp> {
    std::fs::read_to_string(stamp(state))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn remember(state: &Service, at: Timestamp) {
    let path = stamp(state);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = flux_core::store::write_atomic(&path, at.to_string().as_bytes(), 0o600);
}

/// "2 hours ago", "yesterday", "3 days ago".
fn ago(then: Timestamp, now: Timestamp) -> String {
    let mins = now.duration_since(then).as_secs().max(0) / 60;
    match mins {
        0..=1 => "a moment ago".into(),
        2..=59 => format!("{mins} minutes ago"),
        60..=119 => "an hour ago".into(),
        120..=1439 => format!("{} hours ago", mins / 60),
        1440..=2879 => "yesterday".into(),
        _ => format!("{} days ago", mins / 1440),
    }
}

/// The note's text when every backup is protected with nothing to look at; `None`
/// otherwise (or when there are no backups).
pub fn all_good(state: &Service, now: Timestamp) -> Option<String> {
    let store = ProfileStore::new(&state.paths);
    let mut newest: Option<Timestamp> = None;
    let mut oldest: Option<Timestamp> = None;
    let mut tested: Option<Timestamp> = None;
    let mut n = 0;
    for listed in store.list().ok()? {
        let p = listed.profile.ok()?;
        if flux_core::profile::has_errors(&p.validate()) {
            return None;
        }
        let id = listed.id;
        if runlog::recent_runs(&state.paths, &id, 1)
            .first()
            .is_some_and(|r| r.status == RunStatus::Failed)
        {
            return None;
        }
        let st = crate::views::status_impl(state, id.to_string()).ok()?;
        if !st.health.is_empty() {
            return None;
        }
        let good = st.last_good?;
        newest = Some(newest.map_or(good, |t| t.max(good)));
        oldest = Some(oldest.map_or(good, |t| t.min(good)));
        if let Some(t) = flux_runner::restore_test::load_state(&state.paths, &id).last_passed {
            tested = Some(tested.map_or(t, |x| x.max(t)));
        }
        n += 1;
    }
    let (newest, oldest) = (newest?, oldest?);
    let mut text = if n == 1 {
        format!(
            "Your backup is protected. The newest copy is from {}.",
            ago(newest, now)
        )
    } else {
        format!(
            "All {n} backups are protected. The newest copy is from {}, the oldest from {}.",
            ago(newest, now),
            ago(oldest, now)
        )
    };
    if let Some(t) = tested {
        text.push_str(&format!(" The last restore test passed {}.", ago(t, now)));
    }
    Some(text)
}

/// Send the weekly note if it's on, due, and everything is fine. Returns what was sent.
/// The first check only starts the week.
pub fn maybe_send(state: &Service, now: Timestamp) -> Option<String> {
    if state.scope() != Scope::User || Settings::load(&state.paths).weekly_note_off {
        return None;
    }
    let Some(last) = last_sent(state) else {
        remember(state, now);
        return None;
    };
    // Due, but not in quiet hours: the next check after them sends it.
    if now.duration_since(last) < WEEK || Settings::load(&state.paths).quiet_now() {
        return None;
    }
    let text = all_good(state, now)?;
    flux_runner::desktop::notify(
        "Your backups are fine",
        &text,
        flux_runner::desktop::Urgency::Normal,
    );
    remember(state, now);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::profile::Target;
    use flux_core::{Paths, ProfileId};

    #[test]
    fn once_a_week_and_only_when_all_is_well() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        std::fs::create_dir_all(t.join("src")).unwrap();
        std::fs::create_dir_all(t.join("tgt")).unwrap();
        std::fs::write(t.join("src/f"), "x").unwrap();
        let paths = Paths::under(t.join("flux"), Scope::User);
        let id = ProfileId::new("p").unwrap();
        let mut p = flux_core::Profile::new(
            "P",
            Target::Local {
                path: t.join("tgt"),
                fs_uuid: None,
            },
            vec![t.join("src")],
        );
        p.options.notify = false;
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let state = Service::new(paths.clone());
        let now = Timestamp::now();
        // Never backed up: not all good.
        assert_eq!(all_good(&state, now), None);
        flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths: &paths,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: flux_runner::Trigger::Manual,
                prune: false,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &flux_core::progress::Progress::new(),
            &flux_core::progress::silent,
        );
        let text = all_good(&state, now).expect("all good");
        assert!(
            text.starts_with("Your backup is protected. The newest copy is from"),
            "{text}"
        );

        // The first check starts the week; nothing until it's over.
        assert_eq!(last_sent(&state), None);
        assert_eq!(maybe_send(&state, now), None);
        assert_eq!(last_sent(&state), Some(now));
        assert_eq!(
            maybe_send(&state, now + SignedDuration::from_hours(24 * 6)),
            None
        );
        // Turned off: never.
        let mut s = Settings::load(&paths);
        s.weekly_note_off = true;
        s.save(&paths).unwrap();
        assert_eq!(maybe_send(&state, now + WEEK), None);
        s.weekly_note_off = false;
        s.save(&paths).unwrap();
        // (Sent for real here would pop up a notification: the text is what's checked.)
        assert!(all_good(&state, now + WEEK).is_some());
    }

    #[test]
    fn durations_in_words() {
        let now = Timestamp::now();
        let h = |n: i64| now - SignedDuration::from_hours(n);
        assert_eq!(ago(now, now), "a moment ago");
        assert_eq!(ago(h(1), now), "an hour ago");
        assert_eq!(ago(h(5), now), "5 hours ago");
        assert_eq!(ago(h(30), now), "yesterday");
        assert_eq!(ago(h(24 * 4), now), "4 days ago");
    }
}
