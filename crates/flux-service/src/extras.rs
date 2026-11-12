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

/// Files previewed are copied out of the backup up to this size.
const PREVIEW_MAX: u64 = 32 << 20;
/// A text file's preview: its first lines, up to this much.
const PREVIEW_TEXT: usize = 64 << 10;
/// …and this many lines.
const PREVIEW_LINES: usize = 400;

/// A look at a file in a snapshot before restoring it.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    /// A picture, as a `data:` URL.
    Image { url: String },
    /// Its first lines; `more`: there's more after them.
    Text { text: String, more: bool },
    /// Neither (or too large): only what the listing says.
    Other { too_large: bool },
}

/// Image types shown in the preview, by file name ending.
fn image_type(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        // Shown as an <img>: its scripts never run.
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// The start of a file's bytes as text: `None` if it isn't (a NUL byte, or not UTF-8
/// short of a character cut at the end). Cut after a line at most `PREVIEW_LINES` lines
/// and `PREVIEW_TEXT` bytes in; `true` if anything was left out.
fn text_start(bytes: &[u8]) -> Option<(String, bool)> {
    let head = &bytes[..bytes.len().min(PREVIEW_TEXT)];
    if head.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        // A character cut by the limit is fine; anything else isn't text.
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&head[..e.valid_up_to()]).ok()?,
        Err(_) => return None,
    };
    let mut more = bytes.len() > head.len();
    let mut end = text.len();
    if let Some((i, _)) = text.match_indices('\n').nth(PREVIEW_LINES - 1) {
        end = i + 1;
        more = true;
    } else if more && let Some(i) = text.rfind('\n') {
        end = i + 1;
    }
    Some((text[..end].to_owned(), more))
}

/// A preview of file `path` in `snapshot`: the picture, the first lines, or neither.
pub fn preview_file_impl(
    state: &Service,
    id: String,
    snapshot: String,
    path: String,
) -> Result<Preview> {
    if state.scope() == Scope::System {
        return Err(ApiError::new(
            "error",
            "Files from system backups can't be previewed here (they're root's); restore them to a folder instead.",
        ));
    }
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snap = repo.find(&snapshot)?;
    let entry = repo
        .entry_at(&snap, path.trim_start_matches('/').as_bytes())?
        .ok_or_else(|| ApiError::new("error", format!("{path} is not in this snapshot")))?;
    if entry.kind != Kind::File {
        return Ok(Preview::Other { too_large: false });
    }
    if entry.size > PREVIEW_MAX {
        return Ok(Preview::Other { too_large: true });
    }
    let local = copy_out(state, &id, &snapshot, &path)?;
    let bytes = std::fs::read(&local)
        .map_err(|e| ApiError::new("error", format!("{}: {e}", local.display())))?;
    // Read: the copy and its folder go (opened copies stay a day; this one isn't open).
    let _ = std::fs::remove_file(&local);
    if let Some(dir) = local.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    if let Some(mime) = image_type(&path) {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        return Ok(Preview::Image {
            url: format!("data:{mime};base64,{data}"),
        });
    }
    Ok(match text_start(&bytes) {
        Some((text, more)) => Preview::Text { text, more },
        None => Preview::Other { too_large: false },
    })
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
        .entry_at(&snap, &want)?
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
    // Webhook addresses (a Slack or Gotify URL is a secret) and headers.
    for w in &mut q.webhooks {
        w.url = format!("{:?} (address hidden)", w.preset).to_lowercase();
        for h in &mut w.headers {
            let name = h.split(':').next().unwrap_or_default().to_owned();
            *h = format!("{name}: (hidden)");
        }
    }
    q
}

/// A message from a run (an event, or the run's error) without what [`redacted`] hides in
/// the profile: a hook's command line and everything it printed, a password command, and
/// webhook addresses and header values. The run log has them in full (it stays on this
/// computer); diagnostics are pasted into bug reports.
fn scrubbed(message: &str, p: &Profile) -> String {
    // "post_backup hook `curl -H 'Authorization: …'` ok:" and the hook's output after it.
    for event in ["pre_backup", "post_backup", "on_error"] {
        if let Some(rest) = message.strip_prefix(&format!("{event} hook `")) {
            let outcome = rest
                .lines()
                .next()
                .and_then(|l| l.rsplit_once("` "))
                .map_or("ran", |(_, o)| o.trim_end_matches(':'));
            return format!("{event} hook (hidden) {outcome}");
        }
    }
    let mut secrets: Vec<&str> = p
        .hooks
        .pre_backup
        .iter()
        .chain(&p.hooks.post_backup)
        .chain(&p.hooks.on_error)
        .map(String::as_str)
        .collect();
    for source in [
        p.vault.as_ref().map(|v| &v.password_source),
        p.borg.as_ref().map(|b| &b.password_source),
    ]
    .into_iter()
    .flatten()
    {
        if let PasswordSource::Command { command } = source {
            secrets.push(command);
        }
    }
    for w in &p.webhooks {
        secrets.push(&w.url);
        secrets.extend(
            w.headers
                .iter()
                .filter_map(|h| h.split_once(':').map(|(_, v)| v.trim())),
        );
    }
    // The longest first, so one that contains another is hidden whole.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = message.to_owned();
    for s in secrets.into_iter().filter(|s| !s.is_empty()) {
        out = out.replace(s, "(hidden)");
    }
    out
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

/// One installed program of Flux Backup, as found on this computer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Component {
    pub path: String,
    /// What `--version` printed (the version number only), or `None` if it didn't run.
    pub version: Option<String>,
}

/// What the About dialog shows: this program's version and the versions of the `flux`
/// command and the helper it would run, which can differ after a partial upgrade.
#[derive(Debug, Clone, Serialize)]
pub struct About {
    pub version: String,
    pub cli: Component,
    pub helper: Option<Component>,
    /// The `flux` command or the helper reports another version than this program.
    pub mismatch: bool,
    pub os: String,
    pub kernel: String,
}

/// The version number in a `--version` line: `flux 0.3.0` → `0.3.0`,
/// `flux-helper 0.3.0 (protocol 3)` → `0.3.0`.
pub fn version_of(line: &str) -> Option<String> {
    line.split_whitespace()
        .nth(1)
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

fn component(path: PathBuf) -> Component {
    let version = std::process::Command::new(&path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| version_of(String::from_utf8_lossy(&o.stdout).lines().next()?));
    Component {
        path: path.display().to_string(),
        version,
    }
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| "unknown".into())
}

fn describe(c: &Component) -> String {
    format!(
        "{} ({})",
        c.version.as_deref().unwrap_or("didn't run"),
        c.path
    )
}

pub fn about_impl() -> About {
    let cli = component(crate::flux_cli_path());
    let helper = crate::client::helper_path().map(component);
    let differs = |c: &Component| {
        c.version
            .as_deref()
            .is_some_and(|v| v != flux_core::VERSION)
    };
    About {
        version: flux_core::VERSION.to_owned(),
        mismatch: differs(&cli) || helper.as_ref().is_some_and(differs),
        cli,
        helper,
        os: os_name(),
        kernel: first_line("uname", &["-r"]),
    }
}

/// A text for bug reports: versions, system, tools, the profile without secrets, and the
/// recent runs with the last one's events. Folder paths are included.
pub fn diagnostics_impl(state: &Service, id: String) -> Result<String> {
    let (pid, p) = state.load(&id)?;
    let os = os_name();
    let about = about_impl();
    let rsync = flux_compat::tools::rsync_status();
    let borg = flux_compat::tools::borg_status();
    let mut out = format!(
        "Flux Backup diagnostics ({})\n\nFlux Backup {}, {} profiles\nflux command: {}\nhelper: {}\nSystem: {os}, kernel {}\nrsync: {}\nborg: {}\nssh: {}\n\n",
        Timestamp::now(),
        flux_core::VERSION,
        match state.scope() {
            Scope::User => "user",
            Scope::System => "system",
        },
        describe(&about.cli),
        about
            .helper
            .as_ref()
            .map(describe)
            .unwrap_or_else(|| "not installed".into()),
        about.kernel,
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
                .map(|e| format!("  error: {}", scrubbed(e, &p)))
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
            let mut e = e.clone();
            match &mut e.event {
                flux_core::progress::Event::Info { message }
                | flux_core::progress::Event::Warning { message, .. } => {
                    *message = scrubbed(message, &p);
                }
                _ => {}
            }
            out.push_str(&format!(
                "{}\n",
                serde_json::to_string(&e).unwrap_or_default()
            ));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    /// "warning" or "danger".
    pub level: &'static str,
    /// What it's about, for filters: `overdue`, `failing`, `low_space`, `unplugged`,
    /// `source_missing`, `destination_failing`, `restore_test_failed`, `unverified`,
    /// `unusual`, `filesystem` (plain folders on a drive that can't hard-link),
    /// `drive_failing` (the backup drive's own health report).
    pub kind: &'static str,
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

/// The latest unusual changes not yet settled ("This was me", or dealt with), from the
/// history. While there are some, the backup is on hold.
pub fn open_unusual(
    paths: &flux_core::Paths,
    id: &ProfileId,
    runs: &[RunRecord],
) -> Option<flux_runner::unusual::UnusualReport> {
    flux_runner::open_unusual(paths, id, runs)
}

/// Unusual changes in words.
pub fn unusual_health(u: &flux_runner::unusual::UnusualReport) -> Health {
    Health {
        kind: "unusual",
        level: "danger",
        text: if u.stopped {
            format!(
                "Backups are on hold: Flux Sentry stopped the last backup before it was written. Unusual changes in the files: {}.",
                u.signals.join("; ")
            )
        } else {
            format!(
                "Backups are on hold: Flux Sentry found unusual changes in snapshot {}: {}.",
                u.snapshot,
                u.signals.join("; ")
            )
        },
        hint: Some(
            "If you didn't make them, don't delete anything: the snapshot before them is kept, and no backup is made until you've restored what you need and said it's dealt with. If you did, choose “This was me”."
                .into(),
        ),
    }
}

/// Restore tests in words: the last one failed, or a passing one is overdue.
pub fn restore_test_health(
    p: &Profile,
    state: &flux_runner::restore_test::TestState,
    runs: &[RunRecord],
    now: Timestamp,
) -> Vec<Health> {
    use flux_runner::restore_test::{self, TestResult};
    let mut out = Vec::new();
    if let Some(last) = state.history.last()
        && last.result == TestResult::Failed
    {
        out.push(Health {
            kind: "restore_test_failed",
            level: "danger",
            text: format!(
                "The last restore test failed: {}.",
                last.error.clone().unwrap_or_else(|| {
                    last.problems
                        .iter()
                        .find(|x| x.problem.is_failure())
                        .map(|x| format!("{} {}", x.path, x.problem.words()))
                        .unwrap_or_else(|| "some files have problems".into())
                })
            ),
            hint: Some(
                "Check the backup (Overview › Check backups), and run another backup.".into(),
            ),
        });
    } else {
        let oldest_good = runs
            .iter()
            .rev()
            .find(|r| {
                matches!(
                    r.status,
                    RunStatus::Completed | RunStatus::CompletedWithErrors
                )
            })
            .map(|r| r.finished);
        if let Some(days) = restore_test::overdue(&p.restore_test, state, oldest_good, now) {
            out.push(Health {
                kind: "unverified",
                level: "warning",
                text: if state.last_passed.is_some() {
                    format!("No restore test has passed for {days} days.")
                } else {
                    "No restore test has passed yet.".into()
                },
                hint: Some("Test a restore now, from the Overview.".into()),
            });
        }
    }
    out
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
            kind: "unplugged",
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

    // A source's drive missed the last backups (they kept, or left out, its files).
    for src in &p.sources.paths {
        let missed = runs
            .iter()
            .filter(|r| !matches!(r.status, RunStatus::Skipped | RunStatus::DryRun))
            .take_while(|r| r.sources_missing.contains(src))
            .count();
        if missed == 0 {
            continue;
        }
        let name = src.file_name().map_or_else(
            || src.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let kept = matches!(p.engine, flux_core::Engine::Vault)
            || (p.engine == flux_core::Engine::Mirror && matches!(p.target, Target::Local { .. }));
        out.push(Health {
            kind: "source_missing",
            level: if missed >= 3 { "danger" } else { "warning" },
            text: format!(
                "The drive with {name} wasn't connected for the last {}. {}",
                if missed == 1 {
                    "backup".to_owned()
                } else {
                    format!("{missed} backups")
                },
                if kept {
                    "Its files were kept as they were before; changes since then aren't backed up."
                } else {
                    "Those snapshots don't have it; earlier ones still do."
                }
            ),
            hint: Some(format!(
                "Connect the drive with {} before the next backup.",
                src.display()
            )),
        });
    }

    // A destination that keeps failing (the main one is judged below).
    for d in &p.destinations {
        let results: Vec<_> = runs
            .iter()
            .filter(|r| !matches!(r.status, RunStatus::Skipped | RunStatus::DryRun))
            .filter_map(|r| r.destinations.iter().find(|x| x.name == d.name))
            .collect();
        let failed = results
            .iter()
            .take_while(|x| x.status == RunStatus::Failed)
            .count();
        if failed > 0 {
            // A warning: the other copies are still made (the main one is judged below).
            out.push(Health {
                kind: "destination_failing",
                level: "warning",
                // The error itself is in Activity and on the place's row: the sentence
                // says what happened and what to do.
                text: format!(
                    "The copy on {} failed {}.",
                    d.label(),
                    if failed == 1 {
                        "in the last backup".to_owned()
                    } else {
                        format!("in the last {failed} backups")
                    },
                ),
                hint: Some(format!(
                    "Check that {} is reachable; the other places are fine.",
                    d.label()
                )),
            });
        }
    }

    // Repeated failures.
    let failed = runs
        .iter()
        .take_while(|r| matches!(r.status, RunStatus::Failed))
        .count();
    if failed >= 2 {
        out.push(Health {
            kind: "failing",
            level: "danger",
            text: format!("The last {failed} backups failed."),
            hint: Some("Open Activity to see why, or Copy diagnostics to ask for help.".into()),
        });
    }

    // Too long without a successful backup (the unplugged warning already says how long).
    let limit = overdue_after(p);
    match last_ok {
        Some(r) if since(r.finished) > limit && !unplugged => out.push(Health {
            kind: "overdue",
            level: "warning",
            text: format!("No successful backup for {}.", days(since(r.finished))),
            hint: Some(if p.schedule.trigger == Trigger::Manual {
                "This backup only runs when you start it. Back up now, or give it a schedule in Settings.".into()
            } else {
                "Scheduled backups haven't succeeded lately. Back up now, and check Activity.".into()
            }),
        }),
        None if !runs.is_empty() && failed < 2 => out.push(Health {
            kind: "overdue",
            level: "warning",
            text: "No backup has succeeded yet.".into(),
            hint: Some("Open Activity to see what happened.".into()),
        }),
        _ => {}
    }

    // Plain folders on a drive that can't hard-link: every snapshot a full copy.
    if !unplugged
        && let Some(fit) = flux_core::target_check::filesystem_fit_of(p)
        && fit.severe
    {
        out.push(Health {
            kind: "filesystem",
            level: "warning",
            text: fit.text,
            hint: Some(fit.hint),
        });
    }

    // Little space left.
    if let Some(f) = free
        && f < (5 << 30)
    {
        out.push(Health {
            kind: "low_space",
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
            kind: "low_space",
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

/// What was last seen of a backup's drive health (its main place), for the space card.
#[derive(Debug, Serialize)]
pub struct DriveHealthView {
    /// The drive reports its health at all (most USB sticks and many enclosures don't).
    pub reports: bool,
    pub checked: Option<jiff::Timestamp>,
    /// What's wrong, when something is (also among the health warnings).
    pub said: Option<String>,
}

/// The health warnings from what the backup drives last said about themselves (each
/// place's: `failing` danger, signs of wear a warning), and the main place's view.
pub fn drive_health(
    paths: &flux_core::Paths,
    id: &str,
    p: &Profile,
    now: Timestamp,
) -> (Vec<Health>, Option<DriveHealthView>) {
    use flux_runner::drive_health;
    let mut out = Vec::new();
    let mut view = None;
    for (i, (key, _)) in drive_health::places(id, p).into_iter().enumerate() {
        let Some(k) = drive_health::load(paths, &key) else {
            continue;
        };
        let said = k.said(now);
        if let Some(s) = &said {
            out.push(Health {
                kind: "drive_failing",
                level: if s.danger { "danger" } else { "warning" },
                text: s.text.clone(),
                hint: Some(s.hint.clone()),
            });
        }
        if i == 0 {
            view = Some(DriveHealthView {
                reports: k.reports,
                checked: k.checked,
                said: said.map(|s| s.text),
            });
        }
    }
    (out, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_runner::Trigger as RunTrigger;

    #[test]
    fn a_preview_from_a_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let src = t.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(t.join("tgt")).unwrap();
        std::fs::write(src.join("notes.md"), "# Notes\nbuy milk\n").unwrap();
        // A 1×1 PNG's start is enough: it's shown as a picture by its name.
        std::fs::write(src.join("dot.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(src.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let paths = flux_core::Paths::under(t.join("home"), Scope::User);
        let id = ProfileId::new("p").unwrap();
        let mut p = Profile::new(
            "P",
            Target::Local {
                path: t.join("tgt"),
                fs_uuid: None,
            },
            vec![src.clone()],
        );
        p.options.notify = false;
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let snap = flux_runner::run_backup(
            &flux_runner::BackupJob {
                paths: &paths,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: RunTrigger::Manual,
                prune: false,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        )
        .record
        .snapshot
        .unwrap();
        let state = Service::new(paths);
        let at = |name: &str| src.join(name).display().to_string();
        let look =
            |name: &str| preview_file_impl(&state, "p".into(), snap.clone(), at(name)).unwrap();
        assert_eq!(
            look("notes.md"),
            Preview::Text {
                text: "# Notes\nbuy milk\n".into(),
                more: false
            }
        );
        assert_eq!(
            look("dot.png"),
            Preview::Image {
                url: "data:image/png;base64,iVBORw0KGgo=".into()
            }
        );
        assert_eq!(look("blob.bin"), Preview::Other { too_large: false });
        // A folder has no preview.
        assert_eq!(
            preview_file_impl(&state, "p".into(), snap.clone(), src.display().to_string()).unwrap(),
            Preview::Other { too_large: false }
        );
        // Nothing is left behind in the cache.
        let opened = state.paths.cache_dir.join("opened");
        let left: usize = std::fs::read_dir(&opened)
            .map(|rd| {
                rd.flatten()
                    .map(|d| std::fs::read_dir(d.path()).map(|r| r.count()).unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0);
        assert_eq!(left, 0, "the copies are removed");
    }

    #[test]
    fn previews_text_and_pictures() {
        assert_eq!(image_type("/home/me/Pictures/cat.JPG"), Some("image/jpeg"));
        assert_eq!(image_type("/home/me/logo.svg"), Some("image/svg+xml"));
        assert_eq!(image_type("/home/me/notes.md"), None);
        assert_eq!(
            text_start(b"one\ntwo\n"),
            Some(("one\ntwo\n".into(), false))
        );
        assert_eq!(text_start(b"\x00\x01binary"), None);
        assert_eq!(text_start(&[0xff, 0xfe, b'a']), None, "not UTF-8");
        // Long: cut after a whole line, and said to go on.
        let long = "a line of text\n".repeat(10_000);
        let (t, more) = text_start(long.as_bytes()).unwrap();
        assert!(more);
        assert_eq!(t.lines().count(), PREVIEW_LINES);
        assert!(t.ends_with('\n'));
        // A character cut by the size limit is still text.
        let mut wide = "é".repeat(PREVIEW_TEXT / 2);
        wide.push_str("tail");
        let cut = text_start(&wide.as_bytes()[..PREVIEW_TEXT + 1]).unwrap();
        assert!(cut.1);
    }

    #[test]
    fn a_failing_copy_is_a_warning() {
        let mut p = Profile::new(
            "t",
            Target::Local {
                path: "/nonexistent-but-unchecked".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.destinations.push(flux_core::profile::Destination {
            name: "nas".into(),
            label: Some("Office NAS".into()),
            engine: flux_core::Engine::Rsync,
            target: Target::Local {
                path: "/srv".into(),
                fs_uuid: None,
            },
            offsite: true,
            mode: Default::default(),
            vault: None,
            rsync: None,
            borg: None,
        });
        let with = |status: RunStatus| {
            let mut r = run(RunStatus::Completed, 0);
            r.destinations = vec![flux_runner::runlog::DestinationRun {
                name: "nas".into(),
                status,
                snapshot: None,
                error: Some("no route to host.".into()),
                stats: None,
            }];
            r
        };
        let now = NOW.with(|n| *n);
        let h = health(
            &p,
            &[
                with(RunStatus::Failed),
                with(RunStatus::Failed),
                with(RunStatus::Completed),
            ],
            None,
            None,
            now,
        );
        let copy: Vec<_> = h.iter().filter(|x| x.text.contains("Office NAS")).collect();
        assert_eq!(copy.len(), 1, "{h:?}");
        assert_eq!(copy[0].level, "warning");
        assert_eq!(
            copy[0].text,
            "The copy on Office NAS failed in the last 2 backups."
        );
        let h = health(&p, &[with(RunStatus::Completed)], None, None, now);
        assert!(!h.iter().any(|x| x.text.contains("Office NAS")));
    }

    #[test]
    fn a_source_drive_missing_is_said() {
        let src = std::path::PathBuf::from("/mnt/photos");
        let mut p = Profile::new(
            "t",
            Target::Ssh {
                user: None,
                host: "nas".into(),
                port: None,
                path: "/srv".into(),
                identity_file: None,
                ssh_options: Vec::new(),
                bandwidth_limit: None,
            },
            vec!["/home/me".into(), src.clone()],
        );
        p.engine = flux_core::Engine::Vault;
        let missed = |n: usize| -> Vec<RunRecord> {
            let mut v: Vec<RunRecord> = (0..5)
                .map(|i| {
                    let mut r = run(RunStatus::Completed, i);
                    if (i as usize) < n {
                        r.sources_missing = vec![src.clone()];
                    }
                    r
                })
                .collect();
            v.insert(0, run(RunStatus::Skipped, 0)); // skips don't count either way
            v
        };
        let now = NOW.with(|n| *n);
        assert!(health(&p, &missed(0), None, None, now).is_empty());
        let h = health(&p, &missed(1), None, None, now);
        assert_eq!(h.len(), 1);
        assert!(
            h[0].text
                .starts_with("The drive with photos wasn't connected for the last backup."),
            "{}",
            h[0].text
        );
        assert!(h[0].text.contains("kept as they were"));
        let h = health(&p, &missed(3), None, None, now);
        assert_eq!(h[0].level, "danger");
        assert!(h[0].text.contains("last 3 backups"));
    }

    #[test]
    fn version_lines() {
        assert_eq!(version_of("flux 0.3.0").as_deref(), Some("0.3.0"));
        assert_eq!(
            version_of("flux-helper 1.0.0 (protocol 3)").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(version_of("flux"), None);
        assert_eq!(version_of("usage: flux [OPTIONS]"), None);
    }

    #[test]
    fn components_report_their_version_or_none() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("flux");
        let bad = tmp.path().join("broken");
        assert!(flux_transport::testing::executable(
            &good,
            "#!/bin/sh\necho 'flux 9.9.9'\n"
        ));
        assert!(flux_transport::testing::executable(
            &bad,
            "#!/bin/sh\nexit 1\n"
        ));
        let c = component(good.clone());
        assert_eq!(c.version.as_deref(), Some("9.9.9"));
        assert_eq!(c.path, good.display().to_string());
        assert_eq!(component(bad).version, None);
        assert_eq!(component(tmp.path().join("missing")).version, None);
    }

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
            sources_missing: Vec::new(),
            destinations: Vec::new(),
            unusual: None,
            sentry_seen: None,
            performance: None,
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
        assert_eq!(h[0].kind, "overdue", "for the Missed filter");
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
        assert!(
            h.iter()
                .any(|x| x.text == "The last 2 backups failed." && x.kind == "failing")
        );
        let h = health(
            &p,
            &[run(RunStatus::Completed, 0)],
            Some(512 << 20),
            None,
            now,
        );
        assert_eq!((h[0].level, h[0].kind), ("danger", "low_space"));
        p.target = Target::Local {
            path: "/nonexistent/BackupDisk".into(),
            fs_uuid: None,
        };
        let h = health(&p, &[run(RunStatus::Completed, 0)], None, None, now);
        assert!(
            h[0].text.starts_with("BackupDisk isn't connected") && h[0].kind == "unplugged",
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
        p.webhooks = vec![flux_core::webhook::Webhook {
            url: "https://hooks.slack.com/services/T0/B0/XYZsecret".into(),
            preset: flux_core::webhook::Preset::Slack,
            events: Vec::new(),
            headers: vec!["Authorization: Bearer topsecret".into()],
            enabled: true,
        }];
        p.vault = Some(flux_core::profile::VaultOptions {
            password_source: PasswordSource::Command {
                command: "echo hunter2".into(),
            },
            ..Default::default()
        });
        let t = redacted(&p).to_toml().unwrap();
        assert!(!t.contains("XYZsecret") && !t.contains("topsecret"), "{t}");
        assert!(t.contains("Authorization: (hidden)"));
        assert!(!t.contains("secret") && !t.contains("hunter2"), "{t}");
        // What a run says is scrubbed of the same things.
        for (said, shown) in [
            (
                "pre_backup hook `curl -H 'token: secret' …` ok:\ntoken accepted: secret",
                "pre_backup hook (hidden) ok",
            ),
            (
                "on_error hook `curl -H 'token: secret' …` failed:\n",
                "on_error hook (hidden) failed",
            ),
            (
                "webhook https://hooks.slack.com/services/T0/B0/XYZsecret: timed out",
                "webhook (hidden): timed out",
            ),
            (
                "password command failed: `echo hunter2` exited with 1",
                "password command failed: `(hidden)` exited with 1",
            ),
            ("copied 12 files", "copied 12 files"),
        ] {
            assert_eq!(scrubbed(said, &p), shown);
        }
    }
}
