//! Automatic backups through cron, for people who'd rather not use systemd timers.
//!
//! Flux keeps one block in your crontab (`crontab -l`), or for system profiles the file
//! `/etc/cron.d/flux-backup`, and never touches lines outside it:
//!
//! ```text
//! # BEGIN Flux Backup: generated; change schedules in Flux, not here
//! # profile home
//! 0 7 * * * … flux backup home --scheduled --if-due >/dev/null 2>&1
//! */15 * * * * … flux backup home --scheduled --if-due >/dev/null 2>&1
//! # END Flux Backup
//! ```
//!
//! Every line runs `flux backup --if-due`, and Flux decides. A calendar schedule fires at
//! its times, and (with catch-up on) a check every 15 minutes runs a backup whose time
//! passed while the computer was off or asleep, as systemd's `Persistent=` does; the last
//! check is kept in `<state>/schedule/<id>.last`. Interval schedules check every few
//! minutes, as their timers do. Backing up when a drive is plugged in needs systemd.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use jiff::Timestamp;

use flux_core::profile::{Profile, Trigger};
use flux_core::{Paths, ProfileId, Scope, sys};

use crate::calendar::Calendar;
use crate::schedule::{Applied, ScheduleError, ScheduleStatus, UnitEnv, check_period, summary};

const BEGIN: &str = "# BEGIN Flux Backup";
const END: &str = "# END Flux Backup";
const SECTION: &str = "# profile ";
/// How often a calendar schedule with catch-up looks for a missed run.
pub const CATCH_UP_CHECK: &str = "*/15 * * * *";
/// A run this late is "missed", not "on time", when catching up is off.
pub const ON_TIME: Duration = Duration::from_secs(10 * 60);

/// How Flux reads and writes the cron table. Overridable for tests: `FLUX_CRONTAB` (a
/// program called instead of `crontab`) and `FLUX_CRON_DIR` (instead of `/etc/cron.d`).
#[derive(Debug, Clone)]
pub struct Cron {
    pub scope: Scope,
    crontab: String,
    cron_d: PathBuf,
    paths: Paths,
}

/// Lines outside Flux's block, and the block's profiles in order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Table {
    before: Vec<String>,
    entries: Vec<(String, Vec<String>)>,
    after: Vec<String>,
}

impl Table {
    fn parse(text: &str) -> Self {
        let mut t = Table::default();
        let mut state = 0; // 0 before, 1 inside, 2 after
        for line in text.lines() {
            match state {
                0 if line.starts_with(BEGIN) => state = 1,
                0 => t.before.push(line.to_owned()),
                1 if line.starts_with(END) => state = 2,
                1 => {
                    if let Some(id) = line.strip_prefix(SECTION) {
                        t.entries.push((id.trim().to_owned(), Vec::new()));
                    } else if let Some((_, lines)) = t.entries.last_mut()
                        && !line.trim().is_empty()
                        && !line.starts_with('#')
                    {
                        lines.push(line.to_owned());
                    }
                }
                _ => t.after.push(line.to_owned()),
            }
        }
        t
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for l in &self.before {
            let _ = writeln!(out, "{l}");
        }
        if !self.entries.is_empty() {
            let _ = writeln!(
                out,
                "{BEGIN}: generated; change schedules in Flux (or `flux schedule`), not here"
            );
            for (id, lines) in &self.entries {
                let _ = writeln!(out, "{SECTION}{id}");
                for l in lines {
                    let _ = writeln!(out, "{l}");
                }
            }
            let _ = writeln!(out, "{END}");
        }
        for l in &self.after {
            let _ = writeln!(out, "{l}");
        }
        out
    }

    fn get(&self, id: &str) -> Option<&Vec<String>> {
        self.entries.iter().find(|(i, _)| i == id).map(|(_, l)| l)
    }

    /// Set (or with no lines, remove) a profile's lines. Returns whether anything changed.
    fn set(&mut self, id: &str, lines: Vec<String>) -> bool {
        match self.entries.iter().position(|(i, _)| i == id) {
            Some(i) if lines.is_empty() => {
                self.entries.remove(i);
                true
            }
            Some(i) if self.entries[i].1 == lines => false,
            Some(i) => {
                self.entries[i].1 = lines;
                true
            }
            None if lines.is_empty() => false,
            None => {
                self.entries.push((id.to_owned(), lines));
                true
            }
        }
    }
}

/// Quote for `/bin/sh`, and escape `%`, which cron turns into a newline.
fn sh(arg: &str) -> String {
    let q = if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.:=@,+".contains(c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    };
    q.replace('%', "\\%")
}

/// The cron lines a profile needs (none for manual backups).
pub fn lines_for(id: &ProfileId, p: &Profile, env: &UnitEnv) -> Result<Vec<String>, ScheduleError> {
    let s = &p.schedule;
    if s.trigger == Trigger::Manual {
        return Ok(Vec::new());
    }
    let mut cmd = Vec::new();
    if env.scope == Scope::User {
        // cron starts jobs with almost no environment. The session bus is how Flux reaches
        // the keyring and shows notifications; without it (logged out) a backup still runs
        // if it needs neither.
        let run = format!("/run/user/{}", sys::euid());
        cmd.push(format!("XDG_RUNTIME_DIR={run}"));
        cmd.push(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={run}/bus"));
    }
    if let Some(h) = &env.flux_home {
        cmd.push(sh(&format!("FLUX_HOME={}", h.display())));
    }
    cmd.push(sh(&env.flux_bin.display().to_string()));
    if env.scope == Scope::System {
        cmd.push("--system".into());
    }
    cmd.extend([
        "backup".into(),
        sh(id.as_str()),
        "--scheduled".into(),
        "--if-due".into(),
        ">/dev/null".into(),
        "2>&1".into(),
    ]);
    let cmd = cmd.join(" ");
    let user = if env.scope == Scope::System {
        " root"
    } else {
        ""
    };
    let mut times = Vec::new();
    match s.trigger {
        Trigger::Calendar => {
            let expr = s.on_calendar.as_deref().unwrap_or("daily");
            let cal = Calendar::parse(expr).map_err(|e| ScheduleError::BadCalendar {
                expr: expr.into(),
                message: e.to_string(),
            })?;
            times.push(cal.cron_fields());
            if s.catch_up && times[0] != CATCH_UP_CHECK {
                times.push(CATCH_UP_CHECK.into());
            }
        }
        Trigger::Interval => {
            let every = s
                .every
                .map_or(Duration::from_secs(86_400), |e| e.as_duration());
            let mins = (check_period(every).as_secs() / 60).max(1);
            times.push(if mins == 1 {
                "* * * * *".into()
            } else {
                format!("*/{mins} * * * *")
            });
        }
        Trigger::Manual => unreachable!("manual backups returned above"),
    }
    Ok(times
        .into_iter()
        .map(|t| format!("{t}{user} {cmd}"))
        .collect())
}

/// When a calendar schedule last looked for a run to start (its "stamp").
pub fn last_check(paths: &Paths, id: &ProfileId) -> Option<Timestamp> {
    std::fs::read_to_string(stamp_file(paths, id))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn set_last_check(paths: &Paths, id: &ProfileId, t: Timestamp) {
    let f = stamp_file(paths, id);
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = flux_core::store::write_atomic(&f, t.to_string().as_bytes(), 0o644) {
        tracing::warn!("cannot record the schedule check: {e}");
    }
}

fn stamp_file(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths.state_dir.join("schedule").join(format!("{id}.last"))
}

/// Whether cron can be used here: `Err` says what's missing, in words.
pub fn available(scope: Scope) -> Result<(), String> {
    let missing = "cron isn't installed. Install it (on Fedora, the cronie package) and start it, or keep systemd timers.";
    match scope {
        Scope::System => {
            let dir = cron_dir();
            if dir.is_dir() {
                Ok(())
            } else {
                Err(format!("{missing} ({} doesn't exist)", dir.display()))
            }
        }
        Scope::User => match Command::new(crontab_program())
            .arg("-l")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(_) => Ok(()),
            Err(_) => Err(missing.into()),
        },
    }
}

/// Whether a cron daemon is running (`None`: can't tell).
pub fn daemon_running() -> Option<bool> {
    let rd = std::fs::read_dir("/proc").ok()?;
    Some(rd.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("comm"))
            .is_ok_and(|c| matches!(c.trim(), "crond" | "cron" | "cronie" | "fcron" | "dcron"))
    }))
}

fn crontab_program() -> String {
    std::env::var("FLUX_CRONTAB").unwrap_or_else(|_| "crontab".into())
}

fn cron_dir() -> PathBuf {
    std::env::var_os("FLUX_CRON_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/cron.d"))
}

impl Cron {
    pub fn new(paths: &Paths) -> Self {
        Self {
            scope: paths.scope,
            crontab: crontab_program(),
            cron_d: cron_dir(),
            paths: paths.clone(),
        }
    }

    fn system_file(&self) -> PathBuf {
        self.cron_d.join("flux-backup")
    }

    /// What `apply` reports writing.
    fn entry_name(&self) -> String {
        match self.scope {
            Scope::User => "crontab entry".into(),
            Scope::System => format!("{} entry", self.system_file().display()),
        }
    }

    fn read(&self) -> Result<Table, ScheduleError> {
        let text = match self.scope {
            Scope::System => match std::fs::read_to_string(self.system_file()) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(ScheduleError::Io(self.system_file(), e)),
            },
            Scope::User => {
                let out = Command::new(&self.crontab)
                    .arg("-l")
                    .output()
                    .map_err(|e| ScheduleError::Cron(format!("cannot run crontab: {e}")))?;
                let err = String::from_utf8_lossy(&out.stderr);
                if out.status.success() {
                    String::from_utf8_lossy(&out.stdout).into_owned()
                } else if err.to_ascii_lowercase().contains("no crontab") {
                    String::new()
                } else {
                    return Err(ScheduleError::Cron(format!(
                        "`crontab -l` failed: {}",
                        err.trim()
                    )));
                }
            }
        };
        Ok(Table::parse(&text))
    }

    fn write(&self, t: &Table) -> Result<(), ScheduleError> {
        let text = t.render();
        match self.scope {
            Scope::System => {
                let f = self.system_file();
                if t.entries.is_empty() {
                    match std::fs::remove_file(&f) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                            return Err(ScheduleError::Io(f, e));
                        }
                        _ => return Ok(()),
                    }
                }
                let body = format!(
                    "{}\nSHELL=/bin/sh\nPATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n{text}",
                    crate::schedule::MARKER
                );
                flux_core::store::write_atomic(&f, body.as_bytes(), 0o644)
                    .map_err(|e| ScheduleError::Io(f, std::io::Error::other(e)))
            }
            Scope::User => {
                let mut child = Command::new(&self.crontab)
                    .arg("-")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| ScheduleError::Cron(format!("cannot run crontab: {e}")))?;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin
                        .write_all(text.as_bytes())
                        .map_err(|e| ScheduleError::Cron(e.to_string()))?;
                }
                let out = child
                    .wait_with_output()
                    .map_err(|e| ScheduleError::Cron(e.to_string()))?;
                if !out.status.success() {
                    return Err(ScheduleError::Cron(format!(
                        "`crontab -` refused the new table: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                Ok(())
            }
        }
    }

    pub fn apply(
        &self,
        id: &ProfileId,
        p: &Profile,
        env: &UnitEnv,
    ) -> Result<Applied, ScheduleError> {
        let wanted = lines_for(id, p, env)?;
        let mut t = self.read()?;
        let had = t.get(id.as_str()).is_some();
        let mut applied = Applied::default();
        if t.set(id.as_str(), wanted.clone()) {
            self.write(&t)?;
            if wanted.is_empty() {
                applied.removed.push(self.entry_name());
            } else {
                applied.written.push(self.entry_name());
            }
        } else if had {
            applied.unchanged.push(self.entry_name());
        }
        // A new schedule starts now: times before it aren't "missed".
        if p.schedule.trigger == Trigger::Calendar && last_check(&self.paths, id).is_none() {
            set_last_check(&self.paths, id, Timestamp::now());
        }
        Ok(applied)
    }

    pub fn remove(&self, id: &ProfileId) -> Result<Applied, ScheduleError> {
        let mut t = self.read()?;
        let mut applied = Applied::default();
        if t.set(id.as_str(), Vec::new()) {
            self.write(&t)?;
            applied.removed.push(self.entry_name());
        }
        let _ = std::fs::remove_file(stamp_file(&self.paths, id));
        Ok(applied)
    }

    pub fn installed_ids(&self) -> Vec<ProfileId> {
        let mut ids: Vec<ProfileId> = self
            .read()
            .map(|t| t.entries)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, _)| ProfileId::new(id).ok())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn status(&self, id: &ProfileId, p: &Profile, env: &UnitEnv) -> ScheduleStatus {
        let wanted = lines_for(id, p, env).unwrap_or_default();
        let current = self.read().ok().and_then(|t| t.get(id.as_str()).cloned());
        let installed = current.is_some();
        let now = Timestamp::now();
        let tz = jiff::tz::TimeZone::system();
        let next = if !installed {
            None
        } else {
            match p.schedule.trigger {
                Trigger::Calendar => p
                    .schedule
                    .on_calendar
                    .as_deref()
                    .and_then(|e| Calendar::parse(e).ok())
                    .and_then(|c| c.next_after(now, &tz)),
                Trigger::Interval => {
                    let every = p.schedule.every.map(|e| e.as_duration());
                    let last_ok = crate::runlog::recent_runs(&self.paths, id, 200)
                        .into_iter()
                        .find(|r| {
                            use crate::runlog::RunStatus::*;
                            matches!(r.status, Completed | CompletedWithErrors | Unchanged)
                        });
                    match (every, last_ok) {
                        (Some(e), Some(r)) => r.finished.checked_add(e).ok().map(|t| t.max(now)),
                        _ => Some(now),
                    }
                }
                Trigger::Manual => None,
            }
        };
        let show = |t: Timestamp| {
            t.to_zoned(tz.clone())
                .strftime("%a %Y-%m-%d %H:%M:%S %Z")
                .to_string()
        };
        ScheduleStatus {
            profile: id.to_string(),
            schedule: summary(p),
            installed,
            out_of_date: current.as_ref() != Some(&wanted)
                && !(current.is_none() && wanted.is_empty()),
            timer_active: installed,
            next_run: next.map(show),
            next_run_at: next,
            last_trigger: last_check(&self.paths, id).map(show),
        }
    }
}

/// For messages: where the entries are.
pub fn location(scope: Scope) -> String {
    match scope {
        Scope::User => "your crontab".into(),
        Scope::System => Path::new(&cron_dir())
            .join("flux-backup")
            .display()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_core::HumanDuration;
    use flux_core::profile::Target;

    fn env(scope: Scope) -> UnitEnv {
        UnitEnv {
            flux_bin: "/usr/bin/flux".into(),
            scope,
            flux_home: None,
        }
    }

    fn profile(trigger: Trigger) -> Profile {
        let mut p = Profile::new(
            "Home",
            Target::Local {
                path: "/run/media/me/Disk".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.schedule.trigger = trigger;
        p
    }

    #[test]
    fn lines_for_each_kind_of_schedule() {
        let id = ProfileId::new("home").unwrap();
        let mut p = profile(Trigger::Calendar);
        p.schedule.on_calendar = Some("*-*-* 07:00:00".into());
        p.schedule.catch_up = true;
        let l = lines_for(&id, &p, &env(Scope::User)).unwrap();
        let uid = sys::euid();
        let tail = format!(
            "XDG_RUNTIME_DIR=/run/user/{uid} DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus /usr/bin/flux backup home --scheduled --if-due >/dev/null 2>&1"
        );
        assert_eq!(
            l,
            vec![format!("0 7 * * * {tail}"), format!("*/15 * * * * {tail}")]
        );

        p.schedule.catch_up = false;
        assert_eq!(lines_for(&id, &p, &env(Scope::User)).unwrap().len(), 1);

        // System profiles: /etc/cron.d lines name the user, and need no session.
        let l = lines_for(&id, &p, &env(Scope::System)).unwrap();
        assert_eq!(
            l,
            vec!["0 7 * * * root /usr/bin/flux --system backup home --scheduled --if-due >/dev/null 2>&1".to_owned()]
        );

        let mut i = profile(Trigger::Interval);
        i.schedule.every = Some(HumanDuration::from_secs(4 * 3600));
        assert!(
            lines_for(&id, &i, &env(Scope::System)).unwrap()[0].starts_with("*/15 * * * * root ")
        );
        i.schedule.every = Some(HumanDuration::from_secs(300));
        assert!(
            lines_for(&id, &i, &env(Scope::System)).unwrap()[0].starts_with("*/5 * * * * root ")
        );

        assert!(
            lines_for(&id, &profile(Trigger::Manual), &env(Scope::User))
                .unwrap()
                .is_empty()
        );

        p.schedule.on_calendar = Some("daily Europe/Berlin".into());
        assert!(matches!(
            lines_for(&id, &p, &env(Scope::User)),
            Err(ScheduleError::BadCalendar { .. })
        ));
    }

    #[test]
    fn quoting_for_sh_and_cron() {
        assert_eq!(sh("/usr/bin/flux"), "/usr/bin/flux");
        assert_eq!(sh("/opt/my flux/flux"), "'/opt/my flux/flux'");
        assert_eq!(sh("it's"), "'it'\\''s'");
        assert_eq!(sh("/tmp/100%"), "'/tmp/100\\%'");
    }

    #[test]
    fn the_block_leaves_other_lines_alone() {
        let mine = "MAILTO=me\n0 7 * * * /usr/bin/backintime backup --background >/dev/null\n";
        let mut t = Table::parse(mine);
        assert!(t.set("home", vec!["0 7 * * * flux a".into()]));
        assert!(t.set(
            "work",
            vec!["0 8 * * * flux b".into(), "*/15 * * * * flux b".into()]
        ));
        assert!(!t.set("home", vec!["0 7 * * * flux a".into()]), "no change");
        let text = t.render();
        assert!(text.starts_with(mine));
        assert!(text.contains("# profile home\n0 7 * * * flux a\n# profile work\n"));
        // Read back: the same table.
        let back = Table::parse(&text);
        assert_eq!(back, t);
        // Removing the last profile removes the block; the rest is as it was.
        let mut back = back;
        back.set("home", Vec::new());
        back.set("work", Vec::new());
        assert_eq!(back.render(), mine);
        // Lines after the block survive too.
        let around = format!("{text}# mine\n5 5 * * * true\n");
        let mut t = Table::parse(&around);
        t.set("home", vec!["1 1 * * * flux c".into()]);
        assert!(
            t.render()
                .ends_with("# END Flux Backup\n# mine\n5 5 * * * true\n")
        );
    }
}
