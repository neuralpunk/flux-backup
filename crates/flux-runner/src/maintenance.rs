//! Upkeep after a scheduled backup, when it's due: a full verification about once a month
//! (if the profile asks for it) and, for Vault, deleting data no snapshot uses about once a
//! week (backups only forget old snapshots; this is when their space comes back).
//! Problems are notified; nothing here fails the backup.

use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::progress::{Event, Progress, Reporter};
use flux_core::{ByteSize, Engine, Paths, Profile, ProfileId};

use crate::desktop::{self, Action, Urgency};
use crate::password::Ask;
use crate::repo::AnyRepo;

const VERIFY_EVERY: Duration = Duration::from_secs(30 * 86_400);
const FREE_EVERY: Duration = Duration::from_secs(7 * 86_400);

/// When each kind of upkeep last ran (or, the first time, since when it's been counted).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Done {
    #[serde(default)]
    verified: Option<Timestamp>,
    #[serde(default)]
    freed: Option<Timestamp>,
}

fn file(paths: &Paths, id: &ProfileId) -> std::path::PathBuf {
    paths
        .state_dir
        .join("maintenance")
        .join(format!("{id}.json"))
}

/// Is a job last done at `last` due at `now`? The first time, counting starts: nothing is
/// due straight after a profile starts using it.
fn due(last: &mut Option<Timestamp>, every: Duration, now: Timestamp) -> bool {
    match *last {
        None => {
            *last = Some(now);
            false
        }
        Some(t) => now.duration_since(t).as_secs() >= every.as_secs() as i64,
    }
}

/// What's due now for this profile: (verify, free space).
fn what_is_due(p: &Profile, done: &mut Done, now: Timestamp) -> (bool, bool) {
    let verify = p.maintenance.verify_monthly && due(&mut done.verified, VERIFY_EVERY, now);
    let free = p.engine == Engine::Vault
        && p.maintenance.free_space_weekly
        && due(&mut done.freed, FREE_EVERY, now);
    (verify, free)
}

/// Run what's due. Called with the profile's job lock held, after a successful
/// scheduled backup.
pub fn after_backup(
    paths: &Paths,
    id: &ProfileId,
    p: &Profile,
    ask: Option<Ask<'_>>,
    progress: &Progress,
    report: Reporter<'_>,
) {
    let path = file(paths, id);
    let mut done: Done = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let now = Timestamp::now();
    let (verify, free) = what_is_due(p, &mut done, now);
    let info = |message: String| report(Event::Info { message });
    if verify || free {
        match AnyRepo::open(paths, id, p, ask) {
            Err(e) => report(Event::Warning {
                path: String::new(),
                message: format!("scheduled upkeep skipped: {e}"),
            }),
            Ok(repo) => {
                if free {
                    info("weekly upkeep: deleting data no snapshot uses".into());
                    // Only what forgotten snapshots left behind. Which snapshots go is the
                    // backup's decision alone: it knows when to keep them all (after a
                    // run with problems) and keeps the one before an unusual run.
                    match repo.free_unused(report) {
                        Ok(freed) => {
                            info(format!(
                                "weekly upkeep: freed {}",
                                ByteSize(freed.unwrap_or(0)).display_approx()
                            ));
                            done.freed = Some(now);
                        }
                        Err(e) => report(Event::Warning {
                            path: String::new(),
                            message: format!("weekly upkeep failed: {e}"),
                        }),
                    }
                }
                if verify {
                    info("monthly check: reading all stored data back".into());
                    // A backup that keeps repair data has damage rebuilt as it's found.
                    let checked = repo.check_with(true, repo.keeps_repair_data(), progress, report);
                    if let Ok(c) = &checked
                        && !c.repaired.is_empty()
                    {
                        for fixed in &c.repaired {
                            info(format!("monthly check: repaired: {fixed}"));
                        }
                        desktop::notify_with(
                            &format!("Damage in the backups of “{}” was repaired", p.name),
                            &format!(
                                "{} damaged files were rebuilt from their repair data. The drive may be failing: see Flux Backup › Activity.",
                                c.repaired.len()
                            ),
                            Urgency::Normal,
                            id.as_str(),
                            &[Action::Open],
                        );
                    }
                    match checked {
                        Ok(c) if c.errors.is_empty() => {
                            info(format!(
                                "monthly check: {} snapshots, {} read back, all fine",
                                c.snapshots,
                                ByteSize(c.bytes_read).display_approx()
                            ));
                            done.verified = Some(now);
                        }
                        Ok(c) => {
                            for e in &c.errors {
                                report(Event::Warning {
                                    path: String::new(),
                                    message: format!("monthly check: {e}"),
                                });
                            }
                            done.verified = Some(now);
                            desktop::notify_with(
                                &format!("Checking the backups of “{}” found problems", p.name),
                                &format!(
                                    "{} problems. Open Flux Backup › Activity for details.",
                                    c.errors.len()
                                ),
                                Urgency::Critical,
                                id.as_str(),
                                &[Action::Open],
                            );
                        }
                        Err(e) => {
                            report(Event::Warning {
                                path: String::new(),
                                message: format!("monthly check failed: {e}"),
                            });
                            desktop::notify_with(
                                &format!("Couldn't check the backups of “{}”", p.name),
                                &e.to_string(),
                                Urgency::Normal,
                                id.as_str(),
                                &[Action::Open],
                            );
                        }
                    }
                }
            }
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&done) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::Target;

    #[test]
    fn schedule_of_upkeep() {
        let mut p = Profile::new(
            "x",
            Target::Local {
                path: "/mnt".into(),
                fs_uuid: None,
            },
            vec!["/home".into()],
        );
        let now = Timestamp::now();
        let days = |d: i64| now + jiff::SignedDuration::from_hours(24 * d);
        let mut done = Done::default();
        // Mirror, defaults: nothing to do.
        assert_eq!(what_is_due(&p, &mut done, now), (false, false));
        p.engine = Engine::Vault;
        p.maintenance.verify_monthly = true;
        // The first time starts the clock.
        assert_eq!(what_is_due(&p, &mut done, now), (false, false));
        assert_eq!(what_is_due(&p, &mut done, days(6)), (false, false));
        assert_eq!(what_is_due(&p, &mut done, days(7)), (false, true));
        done.freed = Some(days(7));
        assert_eq!(what_is_due(&p, &mut done, days(30)), (true, true));
        p.maintenance.free_space_weekly = false;
        assert_eq!(what_is_due(&p, &mut done, days(30)), (true, false));
    }
}
