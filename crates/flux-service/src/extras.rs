//! Smaller conveniences: opening a file from a snapshot, duplicating a profile,
//! diagnostics for bug reports, and health warnings for the Overview.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use serde::Serialize;

use flux_core::profile::{PasswordSource, Target, Trigger};
use flux_core::progress::{Progress, silent};
use flux_core::{Profile, ProfileId, ProfileStore, Scope};
use flux_fs::entry::Kind;
use flux_fs::fsops::XattrPolicy;
use flux_fs::restore::RestoreRequest;
use flux_fs::safe::LinkPolicy;
use flux_runner::runlog::{self, RunRecord, RunStatus};

use crate::{ApiError, Result, Service};

/// Copies made to open files are removed after this long.
const OPENED_TTL: Duration = Duration::from_secs(24 * 3600);
/// Opening is for looking at a file, not restoring a disk image.
const OPEN_MAX: u64 = 4 << 30;

/// Restore one file of a snapshot into a private temporary folder, read-only, and
/// return its path, so the app can open it in its usual program.
pub fn open_file_impl(
    state: &Service,
    id: String,
    snapshot: String,
    path: String,
) -> Result<String> {
    Ok(copy_out(state, &id, &snapshot, &path)?
        .display()
        .to_string())
}

/// A read-only copy of file `path` from `snapshot`, in a private folder of the cache
/// (cleaned up after a while).
pub fn copy_out(state: &Service, id: &str, snapshot: &str, path: &str) -> Result<PathBuf> {
    let (id, snapshot, path) = (id.to_owned(), snapshot.to_owned(), path.to_owned());
    if state.scope() == Scope::System {
        return Err(ApiError::new(
            "error",
            "Files from system backups can't be opened directly (they're root's); restore them to a folder instead.",
        ));
    }
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snap = repo.find(&snapshot)?;
    let want = path.trim_start_matches('/').as_bytes().to_vec();
    let entry = repo
        .entries(&snap)?
        .filter_map(std::result::Result::ok)
        .find(|e| e.path == want)
        .ok_or_else(|| ApiError::new("error", format!("{path} is not in this snapshot")))?;
    if entry.kind != Kind::File {
        return Err(ApiError::new(
            "error",
            "Only files can be opened; restore folders instead.",
        ));
    }
    if entry.size > OPEN_MAX {
        return Err(ApiError::new(
            "error",
            "That file is too large to open from the backup; restore it to a folder instead.",
        ));
    }
    let base = state.paths.cache_dir.join("opened");
    clean_old(&base);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("{}-{nanos}", snap.id));
    flux_core::store::create_private_dir(&dir)?;
    let req = RestoreRequest {
        snapshot: &snap,
        paths: vec![PathBuf::from(&path)],
        to: Some(dir.clone()),
        keep_existing: false,
        dry_run: false,
        xattrs: XattrPolicy {
            acl: false,
            other: false,
        },
        links: LinkPolicy::for_process(),
        exact: None,
    };
    repo.restore(&req, &Progress::new(), &silent)?;
    let name = Path::new(&path)
        .file_name()
        .ok_or_else(|| ApiError::new("error", "not a file"))?;
    let out = dir.join(name);
    if !out.is_file() {
        return Err(ApiError::new(
            "error",
            "The file couldn't be copied out of the backup.",
        ));
    }
    // Read-only: it's a copy, changes to it wouldn't go anywhere.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o444));
    Ok(out)
}

fn clean_old(base: &Path) {
    let Ok(rd) = std::fs::read_dir(base) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > OPENED_TTL);
        if old {
            let _ = flux_fs::fsops::make_tree_writable(&e.path());
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Copy a profile under a new id ("Home (copy)", `home-copy`), with its timer.
pub fn duplicate_profile_impl(state: &Service, id: String) -> Result<String> {
    let (pid, mut p) = state.load(&id)?;
    p.name = format!("{} (copy)", p.name);
    let store = ProfileStore::new(&state.paths);
    let stem = &pid.as_str()[..pid.as_str().len().min(57)];
    let mut n = 1;
    let new = loop {
        let candidate = if n == 1 {
            format!("{stem}-copy")
        } else {
            format!("{stem}-copy-{n}")
        };
        let c = ProfileId::new(candidate)?;
        if !store.exists(&c) {
            break c;
        }
        n += 1;
    };
    store.create(&new, &p)?;
    crate::edit::apply_timer(state, &new, &p);
    Ok(new.to_string())
}

/// A profile with what might be secret left out: password commands, hook commands.
fn redacted(p: &Profile) -> Profile {
    let mut q = p.clone();
    let hide = |s: &mut PasswordSource| {
        if let PasswordSource::Command { command } = s {
            *command = "(hidden)".into();
        }
    };
    if let Some(v) = q.vault.as_mut() {
        hide(&mut v.password_source);
    }
    if let Some(b) = q.borg.as_mut() {
        hide(&mut b.password_source);
    }
    for list in [
        &mut q.hooks.pre_backup,
        &mut q.hooks.post_backup,
        &mut q.hooks.on_error,
    ] {
        for c in list.iter_mut() {
            *c = "(hidden)".into();
        }
    }
    q
}

fn first_line(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|o| {
            let text = if o.stdout.is_empty() {
                o.stderr
            } else {
                o.stdout
            };
            String::from_utf8_lossy(&text)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_owned()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "not found".into())
}

/// A text for bug reports: versions, system, tools, the profile without secrets, and the
/// recent runs with the last one's events. Folder paths are included.
pub fn diagnostics_impl(state: &Service, id: String) -> Result<String> {
    let (pid, p) = state.load(&id)?;
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| "unknown".into());
    let rsync = flux_compat::tools::rsync_status();
    let borg = flux_compat::tools::borg_status();
    let mut out = format!(
        "Flux Backup diagnostics ({})\n\nFlux Backup {}, {} profiles\nSystem: {os}, kernel {}\nrsync: {}\nborg: {}\nssh: {}\n\n",
        Timestamp::now(),
        flux_core::VERSION,
        match state.scope() {
            Scope::User => "user",
            Scope::System => "system",
        },
        first_line("uname", &["-r"]),
        rsync.version.as_deref().unwrap_or("not found"),
        borg.version.as_deref().unwrap_or("not found"),
        first_line("ssh", &["-V"]),
    );
    out.push_str(&format!(
        "--- profile {pid} (password commands and hooks hidden) ---\n{}\n",
        redacted(&p).to_toml().unwrap_or_default()
    ));
    let issues = p.validate_in(state.scope());
    if !issues.is_empty() {
        out.push_str("--- problems ---\n");
        for i in &issues {
            out.push_str(&format!("{i}\n"));
        }
        out.push('\n');
    }
    let runs = runlog::recent_runs(&state.paths, &pid, 10);
    out.push_str("--- recent runs ---\n");
    for r in &runs {
        out.push_str(&format!(
            "{}  {:?}  {}{}\n",
            r.started,
            r.status,
            r.snapshot.as_deref().unwrap_or("-"),
            r.error
                .as_deref()
                .map(|e| format!("  error: {e}"))
                .unwrap_or_default()
        ));
    }
    if let Some(last) = runs.first().filter(|r| !r.run_id.is_empty()) {
        out.push_str(&format!(
            "\n--- events of the last run ({}) ---\n",
            last.run_id
        ));
        let events = runlog::run_events(&state.paths, &pid, &last.run_id);
        let skip = events.len().saturating_sub(80);
        for e in events.iter().skip(skip) {
            out.push_str(&format!(
                "{}\n",
                serde_json::to_string(e).unwrap_or_default()
            ));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    /// "warning" or "danger".
    pub level: &'static str,
    pub text: String,
    /// What to do about it.
    pub hint: Option<String>,
}

/// How long without a successful backup is worth mentioning, for this schedule.
fn overdue_after(p: &Profile) -> Duration {
    const DAY: u64 = 86_400;
    let s = &p.schedule;
    Duration::from_secs(match s.trigger {
        Trigger::Manual => 30 * DAY,
        Trigger::Interval => s.every.map_or(3 * DAY, |e| (e.0.as_secs() * 3).max(3600)),
        Trigger::Calendar => match s.on_calendar.as_deref().unwrap_or("").trim() {
            "hourly" => DAY,
            c if c.contains("Mon..Fri") => 4 * DAY,
            c if c.starts_with("*-*-*") || c == "daily" => 3 * DAY,
            c if ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun", "weekly"]
                .iter()
                .any(|d| c.starts_with(d)) =>
            {
                14 * DAY
            }
            _ => 14 * DAY,
        },
    })
}

fn days(d: Duration) -> String {
    let n = d.as_secs() / 86_400;
    match n {
        0 => format!("{} hours", (d.as_secs() / 3600).max(1)),
        1 => "1 day".into(),
        n => format!("{n} days"),
    }
}

/// Things worth knowing about a profile, in words: no recent backup, the drive not
/// connected, repeated failures, little space left.
pub fn health(
    p: &Profile,
    runs: &[RunRecord],
    free: Option<u64>,
    days_left: Option<u32>,
    now: Timestamp,
) -> Vec<Health> {
    let mut out = Vec::new();
    let since = |t: Timestamp| Duration::from_secs(now.duration_since(t).as_secs().max(0) as u64);
    // A backup with some files skipped still happened.
    let last_ok = runs.iter().find(|r| {
        matches!(
            r.status,
            RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Unchanged
        )
    });

    // The drive isn't there.
    let mut unplugged = false;
    if let Target::Local { path, fs_uuid } = &p.target
        && flux_core::target_check::check_local_target(path, fs_uuid.as_deref()).is_err()
    {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        out.push(Health {
            level: "warning",
            text: match last_ok {
                Some(r) => format!(
                    "{name} isn't connected. The last backup to it was {} ago.",
                    days(since(r.finished))
                ),
                None => format!("{name} isn't connected."),
            },
            hint: Some(format!("Plug in {name} to back up.")),
        });
        unplugged = true;
    }

    // Repeated failures.
    let failed = runs
        .iter()
        .take_while(|r| matches!(r.status, RunStatus::Failed))
        .count();
    if failed >= 2 {
        out.push(Health {
            level: "danger",
            text: format!("The last {failed} backups failed."),
            hint: Some("Open Activity to see why, or Copy diagnostics to ask for help.".into()),
        });
    }

    // Too long without a successful backup (the unplugged warning already says how long).
    let limit = overdue_after(p);
    match last_ok {
        Some(r) if since(r.finished) > limit && !unplugged => out.push(Health {
            level: "warning",
            text: format!("No successful backup for {}.", days(since(r.finished))),
            hint: Some(if p.schedule.trigger == Trigger::Manual {
                "This backup only runs when you start it. Back up now, or give it a schedule in Settings.".into()
            } else {
                "Scheduled backups haven't succeeded lately. Back up now, and check Activity.".into()
            }),
        }),
        None if !runs.is_empty() && failed < 2 => out.push(Health {
            level: "warning",
            text: "No backup has succeeded yet.".into(),
            hint: Some("Open Activity to see what happened.".into()),
        }),
        _ => {}
    }

    // Little space left.
    if let Some(f) = free
        && f < (5 << 30)
    {
        out.push(Health {
            level: if f < (1 << 30) { "danger" } else { "warning" },
            text: format!(
                "Only {} free where the backups go.",
                flux_core::ByteSize(f).display_approx()
            ),
            hint: Some(
                "Free up space (Maintenance), keep fewer snapshots, or use a bigger drive.".into(),
            ),
        });
    } else if let Some(d) = days_left
        && d < 30
    {
        out.push(Health {
            level: "warning",
            text: format!(
                "At the rate it's been filling, the backup drive is full in about {}.",
                days(Duration::from_secs(u64::from(d.max(1)) * 86_400))
            ),
            hint: Some(
                "Free up space (Maintenance), keep fewer snapshots, or use a bigger drive.".into(),
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_runner::Trigger as RunTrigger;

    thread_local! {
        /// One clock for a whole test.
        static NOW: Timestamp = Timestamp::now();
    }

    fn run(status: RunStatus, ago_days: i64) -> RunRecord {
        let t = NOW.with(|n| *n) - jiff::SignedDuration::from_hours(24 * ago_days);
        RunRecord {
            run_id: "r".into(),
            profile: "p".into(),
            trigger: RunTrigger::Scheduled,
            started: t,
            finished: t,
            status,
            snapshot: None,
            pruned: vec![],
            error: None,
            stats: None,
            warnings: 0,
            hooks: vec![],
            free_after: None,
        }
    }

    #[test]
    fn health_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Profile::new(
            "x",
            Target::Local {
                path: dir.path().to_owned(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.schedule.trigger = Trigger::Calendar;
        p.schedule.on_calendar = Some("daily".into());
        let now = NOW.with(|n| *n);
        assert!(
            health(
                &p,
                &[run(RunStatus::Completed, 1)],
                Some(100 << 30),
                None,
                now
            )
            .is_empty()
        );
        let h = health(
            &p,
            &[run(RunStatus::Completed, 9)],
            Some(100 << 30),
            None,
            now,
        );
        assert_eq!(h.len(), 1);
        assert!(h[0].text.contains("9 days"), "{:?}", h);
        let h = health(
            &p,
            &[
                run(RunStatus::Failed, 0),
                run(RunStatus::Failed, 1),
                run(RunStatus::Completed, 2),
            ],
            Some(100 << 30),
            None,
            now,
        );
        assert!(h.iter().any(|x| x.text == "The last 2 backups failed."));
        let h = health(
            &p,
            &[run(RunStatus::Completed, 0)],
            Some(512 << 20),
            None,
            now,
        );
        assert_eq!(h[0].level, "danger");
        p.target = Target::Local {
            path: "/nonexistent/BackupDisk".into(),
            fs_uuid: None,
        };
        let h = health(&p, &[run(RunStatus::Completed, 0)], None, None, now);
        assert!(
            h[0].text.starts_with("BackupDisk isn't connected"),
            "{:?}",
            h
        );
        // Filling up: when, in words.
        p.target = Target::Local {
            path: "/".into(),
            fs_uuid: None,
        };
        let h = health(
            &p,
            &[run(RunStatus::Completed, 0)],
            Some(100 << 30),
            Some(12),
            now,
        );
        assert!(h[0].text.contains("full in about 12 days"), "{:?}", h);
        p.target = Target::Local {
            path: "/nonexistent/BackupDisk".into(),
            fs_uuid: None,
        };
        // Unplugged for a while: one warning, not two saying the same thing.
        let h = health(&p, &[run(RunStatus::Completed, 9)], None, None, now);
        assert_eq!(h.len(), 1, "{:?}", h);
    }

    #[test]
    fn redaction() {
        let mut p = Profile::new(
            "x",
            Target::Local {
                path: "/mnt".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.hooks.pre_backup = vec!["curl -H 'token: secret' …".into()];
        p.vault = Some(flux_core::profile::VaultOptions {
            password_source: PasswordSource::Command {
                command: "echo hunter2".into(),
            },
            ..Default::default()
        });
        let t = redacted(&p).to_toml().unwrap();
        assert!(!t.contains("secret") && !t.contains("hunter2"), "{t}");
    }
}
