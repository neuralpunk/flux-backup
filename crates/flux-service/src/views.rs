//! Profile list, overview, snapshots, browsing, unlocking, starting and stopping backups,
//! and restoring.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;

use std::process::{Command, Stdio};
use std::sync::Arc;

use serde::Serialize;
use zeroize::Zeroizing;

use flux_core::profile::{Engine, PasswordSource, Target, has_errors};
use flux_core::{Paths, Profile, ProfileId, ProfileStore, Scope, Severity};
use flux_fs::entry::{Entry, Kind};
use flux_fs::snapshot::SnapshotMeta;
use flux_runner::jobs;
use flux_runner::password;
use flux_runner::runlog::{self, RunRecord};
use flux_runner::schedule;
use flux_vault::VaultError;

use crate::{ApiError, CACHE_SNAPSHOTS, Result, Service};

#[derive(Debug, Serialize)]
pub struct ProfileSummary {
    pub id: String,
    pub scope: Scope,
    name: String,
    engine: Engine,
    target: String,
    remote: bool,
    /// What it backs up, home shown as `~`.
    sources: Vec<String>,
    schedule: String,
    /// Files larger than this are left out ("500MB"); `None` without a limit, and for
    /// Borg, which ignores it.
    size_limit: Option<String>,
    pub errors: Vec<String>,
    last_run: Option<RunRecord>,
    pub running: bool,
}

pub fn target_label(t: &Target) -> String {
    match t {
        Target::Local { path, .. } => path.display().to_string(),
        Target::Ssh {
            host,
            user,
            port,
            path,
            ..
        } => format!(
            "ssh://{}{host}{}/{}",
            user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
            port.map(|p| format!(":{p}")).unwrap_or_default(),
            path.display().to_string().trim_start_matches('/')
        ),
    }
}

fn summary(
    paths: &Paths,
    id: &ProfileId,
    p: &Profile,
    running: &[jobs::JobInfo],
) -> ProfileSummary {
    ProfileSummary {
        id: id.to_string(),
        scope: paths.scope,
        name: p.name.clone(),
        engine: p.engine,
        target: target_label(&p.target),
        remote: matches!(p.target, Target::Ssh { .. }),
        sources: p.sources.paths.iter().map(|s| tilde(s)).collect(),
        schedule: schedule::summary(p),
        size_limit: p
            .sources
            .exclude_larger_than
            .filter(|_| p.engine != Engine::Borg)
            .map(|m| m.to_string()),
        errors: p
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.to_string())
            .collect(),
        last_run: runlog::recent_runs(paths, id, 1).into_iter().next(),
        running: running.iter().any(|j| j.profile == id.as_str()),
    }
}

/// `/home/me/Documents` as `~/Documents` (only the current user's home).
fn tilde(path: &std::path::Path) -> String {
    match flux_core::sys::home_dir()
        .and_then(|h| path.strip_prefix(&h).ok().map(|r| r.to_path_buf()))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub fn profiles_impl(state: &Service) -> Result<Vec<ProfileSummary>> {
    let running = jobs::running(&state.paths);
    let mut out = Vec::new();
    for l in ProfileStore::new(&state.paths).list()? {
        match l.profile {
            Ok(p) => out.push(summary(&state.paths, &l.id, &p, &running)),
            Err(e) => out.push(ProfileSummary {
                id: l.id.to_string(),
                scope: state.paths.scope,
                name: l.id.to_string(),
                engine: Engine::Mirror,
                target: String::new(),
                remote: false,
                sources: Vec::new(),
                schedule: String::new(),
                size_limit: None,
                errors: vec![e.to_string()],
                last_run: None,
                running: false,
            }),
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Overview {
    profile: ProfileSummary,
    snapshot_count: usize,
    oldest: Option<jiff::Timestamp>,
    latest: Option<SnapshotMeta>,
    recent_runs: Vec<RunRecord>,
    /// When the timer fires next (RFC 3339; systemd's display string doesn't parse in
    /// every browser engine).
    next_run: Option<jiff::Timestamp>,
    timer_active: bool,
    free_bytes: Option<u64>,
    /// Why snapshots couldn't be read (e.g. the drive isn't connected).
    repo_error: Option<String>,
    /// Things worth knowing, in words (no recent backup, drive not connected…).
    health: Vec<crate::extras::Health>,
    /// Free space, its history and forecast (local targets).
    space: Option<crate::space::Space>,
}

pub fn overview_impl(state: &Service, id: String) -> Result<Overview> {
    let (id, p) = state.load(&id)?;
    let running = jobs::running(&state.paths);
    let (snaps, repo_error) = match state
        .open(&id, &p)
        .and_then(|r| r.snapshots().map_err(ApiError::from))
    {
        Ok(s) => (s, None),
        Err(e) if e.code == "password_required" => return Err(e),
        Err(e) if e.code == "empty" => (Vec::new(), None),
        Err(e) => (Vec::new(), Some(e.message)),
    };
    let st = schedule::Scheduler::new(&state.paths).status(&id, &p, &state.unit_env());
    let free_bytes = flux_runner::free_space(&p.target);
    let now = jiff::Timestamp::now();
    let runs = runlog::recent_runs(&state.paths, &id, 300);
    let space = crate::space::forecast(&runs, free_bytes, flux_runner::disk_size(&p.target), now);
    let health = crate::extras::health(
        &p,
        &runs,
        free_bytes,
        space.as_ref().and_then(|s| s.days_left),
        now,
    );
    Ok(Overview {
        health,
        space,
        profile: summary(&state.paths, &id, &p, &running),
        snapshot_count: snaps.len(),
        oldest: snaps.first().map(|s| s.started),
        latest: snaps.last().cloned(),
        recent_runs: runs.iter().take(8).cloned().collect(),
        next_run: st.next_run_at,
        timer_active: st.timer_active,
        free_bytes,
        repo_error,
    })
}

/// One day of a backup's history, for the strip of days on the app's overviews.
#[derive(Debug, Serialize)]
pub struct Day {
    /// The local calendar day.
    date: jiff::civil::Date,
    /// `ok` (a backup succeeded that day), `failed` (only failures), or `none`.
    state: &'static str,
    /// Bytes the day's backups copied (Mirror, rsync) or stored (Vault, Borg).
    bytes: u64,
}

/// How a backup is doing, without opening its repository: cheap enough to ask for every
/// backup at once, and it never needs a password.
#[derive(Debug, Serialize)]
pub struct Status {
    next_run: Option<jiff::Timestamp>,
    timer_active: bool,
    /// The last backup that kept a copy (some files skipped still counts).
    last_good: Option<jiff::Timestamp>,
    health: Vec<crate::extras::Health>,
    /// The last [`STATUS_DAYS`] days, oldest first, today last.
    days: Vec<Day>,
}

pub const STATUS_DAYS: usize = 30;

pub fn status_impl(state: &Service, id: String) -> Result<Status> {
    let (id, p) = state.load(&id)?;
    let st = schedule::Scheduler::new(&state.paths).status(&id, &p, &state.unit_env());
    let now = jiff::Timestamp::now();
    let runs = runlog::recent_runs(&state.paths, &id, 300);
    let free = flux_runner::free_space(&p.target);
    let space = crate::space::forecast(&runs, free, flux_runner::disk_size(&p.target), now);
    let health = crate::extras::health(
        &p,
        &runs,
        free,
        space.as_ref().and_then(|s| s.days_left),
        now,
    );
    let tz = jiff::tz::TimeZone::system();
    Ok(Status {
        next_run: st.next_run_at,
        timer_active: st.timer_active,
        last_good: runs
            .iter()
            .find(|r| kept_copy(r.status))
            .map(|r| r.finished),
        health,
        days: days(&runs, p.engine, now.to_zoned(tz.clone()).date(), &tz),
    })
}

fn kept_copy(s: runlog::RunStatus) -> bool {
    use runlog::RunStatus::*;
    matches!(s, Completed | CompletedWithErrors | Unchanged)
}

/// Bucket runs (newest first) into the [`STATUS_DAYS`] days ending `today`.
fn days(
    runs: &[RunRecord],
    engine: Engine,
    today: jiff::civil::Date,
    tz: &jiff::tz::TimeZone,
) -> Vec<Day> {
    let stored = matches!(engine, Engine::Vault | Engine::Borg);
    let mut out: Vec<Day> = (0..STATUS_DAYS as i32)
        .rev()
        .filter_map(|back| today.checked_sub(jiff::Span::new().days(back)).ok())
        .map(|date| Day {
            date,
            state: "none",
            bytes: 0,
        })
        .collect();
    for r in runs {
        let date = r.finished.to_zoned(tz.clone()).date();
        let Some(d) = out.iter_mut().find(|d| d.date == date) else {
            continue;
        };
        if kept_copy(r.status) {
            d.state = "ok";
            d.bytes += r.stats.as_ref().map_or(0, |s| {
                if stored {
                    s.stored_bytes
                } else {
                    s.copied_bytes
                }
            });
        } else if r.status == runlog::RunStatus::Failed && d.state == "none" {
            d.state = "failed";
        }
    }
    out
}

/// The recovery kit of a compatibility-mode profile: how to use its backups without Flux.
#[derive(Debug, Serialize)]
pub struct RecoveryKit {
    engine: Engine,
    /// Where the kit and the backups are kept (a local path or `user@host:path`).
    location: String,
    recovery: String,
    script: String,
    tool: flux_compat::tools::ToolStatus,
}

pub fn recovery_kit_impl(state: &Service, id: String) -> Result<RecoveryKit> {
    let (pid, p) = state.load(&id)?;
    let Some(kit) = flux_compat::kit::for_profile(&pid, &p) else {
        return Err(ApiError::new(
            "error",
            "Recovery kits come with compatibility mode (rsync or Borg).",
        ));
    };
    Ok(RecoveryKit {
        engine: p.engine,
        location: flux_compat::kit::location(&pid, &p),
        recovery: kit.recovery,
        script: kit.script,
        tool: if p.engine == Engine::Borg {
            flux_compat::tools::borg_status()
        } else {
            flux_compat::tools::rsync_status()
        },
    })
}

/// Whether rsync and borg are installed (for choosing compatibility mode).
pub fn tools_impl() -> Vec<flux_compat::tools::ToolStatus> {
    vec![
        flux_compat::tools::rsync_status(),
        flux_compat::tools::borg_status(),
    ]
}

pub fn snapshots_impl(state: &Service, id: String) -> Result<Vec<SnapshotMeta>> {
    let (id, p) = state.load(&id)?;
    Ok(state.open(&id, &p)?.snapshots()?)
}

#[derive(Debug, Serialize)]
pub struct DirEntry {
    name: String,
    path: String,
    kind: Kind,
    size: u64,
    mtime: i64,
    mode: u32,
    link: Option<String>,
    children: Option<usize>,
    /// Whether the item exists at its original place now: `false` when it was deleted
    /// or moved since (worth restoring), `None` when that can't be checked.
    on_disk: Option<bool>,
}

/// Whether `path` exists now, without following a final symlink.
pub(crate) fn on_disk(path: &str) -> Option<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

#[derive(Debug, Serialize)]
pub struct Listing {
    path: String,
    entries: Vec<DirEntry>,
}

fn parent_of(p: &[u8]) -> &[u8] {
    p.iter()
        .rposition(|&b| b == b'/')
        .map_or(&[][..], |i| &p[..i])
}

/// A snapshot's entries with each folder's children, built once when the snapshot is first
/// browsed: opening a folder is then a lookup, not a pass over every entry of the snapshot.
pub struct SnapshotIndex {
    entries: Vec<Entry>,
    /// Folder path (`b""` for the top) → indexes of its children, in entry order.
    folders: HashMap<Vec<u8>, Vec<u32>>,
}

impl SnapshotIndex {
    fn new(entries: Vec<Entry>) -> Self {
        let mut folders: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
        folders.insert(Vec::new(), Vec::new());
        for e in entries.iter().filter(|e| e.kind == Kind::Dir) {
            folders.insert(e.path.clone(), Vec::new());
        }
        for (i, e) in entries.iter().enumerate() {
            // Children of something that isn't a folder of this snapshot stay unreachable.
            if let Some(children) = folders.get_mut(parent_of(&e.path)) {
                children.push(i as u32);
            }
        }
        Self { entries, folders }
    }
}

pub fn list_dir_impl(
    state: &Service,
    id: String,
    snapshot: String,
    path: String,
) -> Result<Listing> {
    let key = (id.clone(), snapshot.clone());
    let cached = {
        let mut c = state.entries.lock().expect("cache");
        // Most recently used first: move a hit to the front.
        c.iter().position(|(k, _)| *k == key).map(|i| {
            let hit = c.remove(i).expect("position is in range");
            let v = hit.1.clone();
            c.push_front(hit);
            v
        })
    };
    let index = match cached {
        Some(v) => v,
        None => {
            let (pid, p) = state.load(&id)?;
            let repo = state.open(&pid, &p)?;
            let snap = repo.find(&snapshot)?;
            let v: Vec<Entry> = repo
                .entries(&snap)?
                .collect::<std::result::Result<_, _>>()?;
            let v = Arc::new(SnapshotIndex::new(v));
            let mut c = state.entries.lock().expect("cache");
            c.push_front((key, v.clone()));
            c.truncate(CACHE_SNAPSHOTS);
            v
        }
    };
    let want = path
        .trim_end_matches('/')
        .trim_start_matches('/')
        .as_bytes()
        .to_vec();
    let Some(children) = index.folders.get(&want) else {
        return Err(ApiError::new(
            "error",
            format!("{path} is not a folder in this snapshot"),
        ));
    };
    let entries = children
        .iter()
        .map(|&i| {
            let e = &index.entries[i as usize];
            let path = format!("/{}", String::from_utf8_lossy(&e.path));
            DirEntry {
                on_disk: on_disk(&path),
                name: String::from_utf8_lossy(
                    &e.path[parent_of(&e.path).len() + usize::from(!want.is_empty())..],
                )
                .into_owned(),
                path,
                kind: e.kind,
                size: e.size,
                mtime: e.mtime,
                mode: e.mode,
                link: e
                    .link
                    .as_ref()
                    .map(|l| String::from_utf8_lossy(l).into_owned()),
                children: (e.kind == Kind::Dir)
                    .then(|| index.folders.get(&e.path).map_or(0, Vec::len)),
            }
        })
        .collect();
    Ok(Listing { path, entries })
}

/// Check a Vault password (or Borg passphrase) and keep it in memory. If the Vault doesn't exist yet, the
/// password becomes the new Vault's when `create` is set (the first backup creates it);
/// otherwise the answer is `new_vault`, so the app can ask for it twice.
pub fn unlock_impl(state: &Service, id: String, password: String, create: bool) -> Result<()> {
    let password = Zeroizing::new(password);
    let (pid, p) = state.load(&id)?;
    if p.engine == Engine::Borg {
        let repo = flux_compat::borg::BorgRepo::for_profile(
            &state.paths,
            &pid,
            &p,
            Some(password.clone()),
            true,
        )
        .map_err(flux_runner::repo::RepoError::from)?;
        if repo.exists() {
            repo.verify().map_err(flux_runner::repo::RepoError::from)?;
        } else if !create {
            return Err(ApiError::new(
                "new_vault",
                "This Borg repository doesn't exist yet. Choose its passphrase; the first backup creates it.",
            ));
        }
        state.remember_password(&pid, password);
        return Ok(());
    }
    let pool = flux_runner::repo::sftp_pool(&state.paths, &p, true);
    match flux_vault::VaultRepo::open(flux_runner::repo::vault_backend(&p, pool), Some(&password)) {
        Ok(_) => {}
        Err(VaultError::NoRepository(_)) if create => {}
        Err(VaultError::NoRepository(_)) => {
            return Err(ApiError::new(
                "new_vault",
                "This Vault doesn't exist yet. Choose its password; the first backup creates it.",
            ));
        }
        Err(e @ VaultError::WrongPassword) => {
            return Err(ApiError::new("wrong_password", e.to_string()));
        }
        Err(e) => return Err(e.into()),
    }
    state.remember_password(&pid, password);
    Ok(())
}

/// Set up encryption while a backup is being set up, so its first run (scheduled or not)
/// needs nobody to type anything: create its encrypted Vault or Borg repository now with
/// `password` (or check that `password` opens the one already there), then keep the
/// password where the profile says it comes from (the desktop keyring, or a key file that
/// doesn't exist yet). `password` may be empty when the profile reads it from a file or a
/// command; then that is what's used.
pub fn set_password_impl(state: &Service, id: String, password: String) -> Result<()> {
    use flux_runner::repo::RepoError;
    let password = Zeroizing::new(password);
    let (pid, p) = state.load(&id)?;
    let Some(source) = p.password_source() else {
        return Err(ApiError::new("error", "This backup isn't encrypted."));
    };
    let typed = !password.is_empty();
    if !typed && matches!(source, PasswordSource::Keyring | PasswordSource::Prompt) {
        return Err(ApiError::new(
            "password_required",
            "Choose a password first.",
        ));
    }
    if typed
        && let PasswordSource::File { path } = &source
        && !path.exists()
    {
        write_secret_file(path, &password)?;
    }

    // Use exactly what was typed (not an older keyring entry): the same profile with the
    // password coming from `ask`.
    let mut with_typed = p.clone();
    if typed {
        match p.engine {
            Engine::Vault => {
                with_typed
                    .vault
                    .get_or_insert_with(Default::default)
                    .password_source = PasswordSource::Prompt
            }
            Engine::Borg => {
                with_typed
                    .borg
                    .get_or_insert_with(Default::default)
                    .password_source = PasswordSource::Prompt
            }
            Engine::Mirror | Engine::Rsync => {}
        }
    }
    let given = password.clone();
    let ask = move |_: &str, _: bool| (!given.is_empty()).then(|| given.clone());
    match p.engine {
        Engine::Vault => {
            flux_runner::repo::open_vault(&state.paths, &pid, &with_typed, true, Some(&ask))?;
        }
        Engine::Borg => {
            flux_compat::tools::require_borg().map_err(RepoError::from)?;
            let repo =
                flux_runner::repo::open_borg(&state.paths, &pid, &with_typed, true, Some(&ask))?;
            if repo.exists() {
                repo.verify().map_err(RepoError::from)?;
            } else {
                repo.init(p.borg.clone().unwrap_or_default().encryption)
                    .map_err(RepoError::from)?;
            }
        }
        Engine::Mirror | Engine::Rsync => {}
    }
    if typed {
        if source == PasswordSource::Keyring {
            password::keyring_store(&pid, &p.name, &password).map_err(RepoError::from)?;
        }
        state.remember_password(&pid, password);
    }
    Ok(())
}

/// Write a password into a new file only its owner can read.
fn write_secret_file(path: &std::path::Path, secret: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(secret.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Would a background `flux backup` of this profile be unable to get its password?
pub fn needs_password(state: &Service, id: &ProfileId, p: &Profile) -> bool {
    let Some(source) = p.password_source() else {
        return false;
    };
    if std::env::var_os("FLUX_PASSWORD").is_some() || state.password(id).is_some() {
        return false;
    }
    match source {
        // Root has no desktop keyring to look in.
        PasswordSource::Keyring if state.scope() == Scope::System => true,
        PasswordSource::Keyring => !matches!(password::keyring_lookup(id), Ok(Some(_))),
        PasswordSource::Prompt => true,
        PasswordSource::File { .. } | PasswordSource::Command { .. } => false,
    }
}

/// Why a local target can't be backed up to right now, in words for people, and whether
/// plugging in a drive would fix it. `None` when it can (or isn't local).
pub fn target_unreachable(p: &Profile) -> Option<(String, bool)> {
    use flux_core::target_check::{TargetError, check_local_target};
    let Target::Local { path, fs_uuid } = &p.target else {
        return None;
    };
    if path.as_os_str().is_empty() {
        return None;
    }
    let err = check_local_target(path, fs_uuid.as_deref()).err()?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let at = path.display();
    Some(match err {
        TargetError::Missing(_) | TargetError::LooksUnmounted(_) => (
            format!("{name} isn't connected: nothing is mounted at {at}."),
            true,
        ),
        TargetError::WrongFilesystem { .. } => (
            format!("Another drive is mounted at {at}, not the one this backup uses."),
            true,
        ),
        TargetError::NotADirectory(_) => (format!("{at} is a file, not a folder."), false),
    })
}

/// How a backup start went.
#[derive(Debug, Serialize)]
pub struct Started {
    /// Still running: its progress follows through the job watcher. `false` when it was
    /// already done by the time this returned.
    pub running: bool,
}

/// How long to wait for a new backup to show up as a running job (or to end).
const START_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Start a backup as a separate `flux backup` process, so it survives closing the window
/// and shows up for every client (GUI, `flux watch`) through its status socket.
///
/// Returns once the job is running, or with its error if it stopped straight away: a backup
/// that fails in its first milliseconds can be over before any watcher connects, and would
/// otherwise look like it never finished.
pub fn backup_now_impl(state: &Service, id: String) -> Result<Started> {
    start_backup(state, id, false)
}

/// [`backup_now_impl`]; with `shutdown`, the computer shuts down a minute after it ends.
pub fn start_backup(state: &Service, id: String, shutdown: bool) -> Result<Started> {
    let (pid, p) = state.load(&id)?;
    let issues = p.validate();
    if has_errors(&issues) {
        return Err(ApiError::new(
            "error",
            "the profile has errors; fix them first",
        ));
    }
    if jobs::running(&state.paths)
        .iter()
        .any(|j| j.profile == pid.as_str())
    {
        return Err(ApiError::new(
            "busy",
            "a backup of this profile is already running",
        ));
    }
    if let Some((why, drive)) = target_unreachable(&p) {
        return Err(ApiError::new(
            "not_connected",
            if drive {
                format!("{why} Plug in the drive, then back up again.")
            } else {
                why
            },
        ));
    }
    if needs_password(state, &pid, &p) {
        return Err(ApiError::new(
            "password_required",
            format!("Enter the password of “{}” to back it up.", p.name),
        ));
    }
    let (program, args) = state
        .backup_cmd
        .split_first()
        .ok_or_else(|| ApiError::new("error", "no backup command configured"))?;
    let mut cmd = Command::new(program);
    cmd.args(args).arg(pid.as_str());
    if shutdown {
        cmd.arg("--shutdown");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(pw) = state.password(&pid) {
        cmd.env("FLUX_PASSWORD", pw.as_str());
    }
    let since = jiff::Timestamp::now();
    let mut child = cmd.spawn().map_err(|e| {
        ApiError::new(
            "error",
            format!("cannot start {}: {e}", program.to_string_lossy()),
        )
    })?;
    // Read stderr as it comes, so a chatty child never blocks on a full pipe.
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        text
    });
    let deadline = std::time::Instant::now() + START_WAIT;
    let exited = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if jobs::running(&state.paths)
            .iter()
            .any(|j| j.profile == pid.as_str() && j.pid == child.id())
            || std::time::Instant::now() >= deadline
        {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let Some(status) = exited else {
        // Running: reap it in the background; its own socket reports progress and outcome.
        std::thread::spawn(move || {
            let status = child.wait();
            let text = reader.join().unwrap_or_default();
            if !matches!(status, Ok(s) if s.success()) {
                tracing::warn!("flux backup exited with {status:?}: {}", text.trim());
            }
        });
        return Ok(Started { running: true });
    };
    let text = reader.join().unwrap_or_default();
    // The run log has the outcome in the same words as Activity; stderr is the fallback
    // (a run that ended before it was recorded, such as an unreadable profile).
    let run = runlog::recent_runs(&state.paths, &pid, 1)
        .into_iter()
        .next()
        .filter(|r| r.started >= since);
    match run {
        Some(r) if r.status == runlog::RunStatus::Failed => Err(ApiError::new(
            "backup_failed",
            r.error.unwrap_or_else(|| "the backup stopped".into()),
        )),
        Some(_) => Ok(Started { running: false }),
        None if status.success() => Ok(Started { running: false }),
        None => {
            let why = text
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .map(|l| l.trim_start_matches("Error: ").to_owned())
                .unwrap_or_else(|| format!("the backup stopped right away ({status})"));
            Err(ApiError::new("backup_failed", why))
        }
    }
}

/// Shut the computer down when a running backup ends (or no longer).
pub fn shutdown_after_impl(state: &Service, id: String, on: bool) -> Result<()> {
    let id = ProfileId::new(id)?;
    let mut client = jobs::Client::connect(&state.paths, &id).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::new("not_running", "This backup isn't running any more.")
        } else {
            ApiError::from(e)
        }
    })?;
    client.shutdown_after(on)?;
    Ok(())
}

/// Cancel a shutdown scheduled after a backup.
pub fn cancel_shutdown_impl() -> Result<()> {
    flux_runner::desktop::cancel_poweroff().map_err(|e| ApiError::new("error", e))
}

pub fn cancel_impl(state: &Service, id: String) -> Result<()> {
    let id = ProfileId::new(id)?;
    let mut client = jobs::Client::connect(&state.paths, &id).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::new("not_running", "This backup isn't running any more.")
        } else {
            ApiError::from(e)
        }
    })?;
    client.cancel()?;
    Ok(())
}

#[cfg(test)]
mod tests {

    fn run(finished: &str, status: &str, copied: u64) -> RunRecord {
        serde_json::from_value(serde_json::json!({
            "run_id": finished, "profile": "p", "trigger": "scheduled",
            "started": finished, "finished": finished, "status": status,
            "stats": {"entries":0,"files":0,"dirs":0,"symlinks":0,"specials":0,"total_bytes":0,
                      "new":0,"changed":0,"unchanged":0,"deleted":0,"copied_bytes":copied,
                      "copied_files":0,"linked_files":0,"stored_bytes":7}
        }))
        .unwrap()
    }

    #[test]
    fn days_bucket_runs_by_local_date() {
        let tz = jiff::tz::TimeZone::UTC;
        let today = jiff::civil::date(2026, 11, 8);
        // Newest first, as the run log returns them.
        let runs = [
            run("2026-11-08T09:00:00Z", "failed", 0),
            run("2026-11-08T01:00:00Z", "completed", 100),
            run("2026-11-07T10:00:00Z", "failed", 0),
            run("2026-11-06T10:00:00Z", "unchanged", 0),
            run("2026-11-06T09:00:00Z", "completed", 50),
            run("2026-09-01T10:00:00Z", "completed", 999), // too old
        ];
        let d = days(&runs, Engine::Mirror, today, &tz);
        assert_eq!(d.len(), STATUS_DAYS);
        assert_eq!(d.last().unwrap().date, today);
        let state = |i: usize| (d[d.len() - 1 - i].state, d[d.len() - 1 - i].bytes);
        assert_eq!(
            state(0),
            ("ok", 100),
            "a success that day wins over a later failure"
        );
        assert_eq!(state(1), ("failed", 0));
        assert_eq!(state(2), ("ok", 50));
        assert_eq!(state(3), ("none", 0));
        // Vault and Borg count what was stored.
        assert_eq!(
            days(&runs, Engine::Borg, today, &tz).last().unwrap().bytes,
            7
        );
    }

    use super::*;
    use flux_core::profile::VaultOptions;
    use flux_core::progress::{Progress, silent};
    use flux_runner::{BackupJob, Trigger, run_backup};
    use std::path::PathBuf;

    struct Env {
        _tmp: tempfile::TempDir,
        state: Service,
        src: PathBuf,
    }

    fn env(engine: Engine) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("docs")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tgt")).unwrap();
        std::fs::write(src.join("docs/a.txt"), "alpha").unwrap();
        std::fs::write(src.join("b.txt"), "beta").unwrap();
        let paths = Paths::under(tmp.path().join("home"), flux_core::Scope::User);
        let mut p = Profile::new(
            "Test",
            Target::Local {
                path: tmp.path().join("tgt"),
                fs_uuid: None,
            },
            vec![src.clone()],
        );
        p.options.notify = false;
        if engine == Engine::Vault {
            p.engine = Engine::Vault;
            p.vault = Some(VaultOptions {
                password_source: PasswordSource::Prompt,
                ..VaultOptions::default()
            });
        }
        if engine == Engine::Rsync {
            p.engine = Engine::Rsync;
        }
        let id = ProfileId::new("t").unwrap();
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let ask = |_: &str, _: bool| Some(Zeroizing::new("pw".to_owned()));
        let r = run_backup(
            &BackupJob {
                paths: &paths,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: Trigger::Manual,
                prune: false,
                only_if_due: false,
                ask: Some(&ask),
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        );
        assert!(r.outcome.is_ok(), "{:?}", r.outcome.err());
        Env {
            _tmp: tmp,
            state: Service::new(paths),
            src,
        }
    }

    /// A file's version in a snapshot compared with the file now: line by line, from a
    /// read-only copy.
    #[test]
    fn compare_a_version_with_the_file_now() {
        let e = env(Engine::Mirror);
        let st = &e.state;
        let id = ProfileId::new("t").unwrap();
        let p = st.load("t").unwrap().1;
        let snap = st.open(&id, &p).unwrap().snapshots().unwrap()[0].id.clone();
        let a = e.src.join("docs/a.txt").display().to_string();
        let d =
            crate::filediff::diff_file_impl(st, "t".into(), a.clone(), snap.clone(), None).unwrap();
        assert!(d.identical, "unchanged since the backup");

        std::fs::write(e.src.join("docs/a.txt"), "alpha\nbeta\n").unwrap();
        let d =
            crate::filediff::diff_file_impl(st, "t".into(), a.clone(), snap.clone(), None).unwrap();
        assert!(!d.identical && !d.binary);
        assert!(d.added >= 1, "{:?}", d.lines);
        assert!(
            d.lines
                .iter()
                .any(|l| l.kind == "added" && l.text == "beta")
        );
        assert_eq!(d.new_file, a, "the file itself, not a copy");
        assert!(
            d.old_file.contains("opened"),
            "a copy from the backup: {}",
            d.old_file
        );

        std::fs::remove_file(e.src.join("docs/a.txt")).unwrap();
        let err = crate::filediff::diff_file_impl(st, "t".into(), a, snap, None).unwrap_err();
        assert!(err.message.contains("isn't on your computer now"));
    }

    /// Exact restore: what the snapshot doesn't have goes (moved aside by default), but
    /// never what the backup doesn't take (exclusions, the size limit) or kept copies.
    #[test]
    fn exact_restore_removes_only_what_the_backup_would_have_taken() {
        let e = env(Engine::Mirror);
        let st = &e.state;
        let src = &e.src;
        let id = ProfileId::new("t").unwrap();
        let mut p = st.load("t").unwrap().1;
        p.sources.exclude = vec!["*.tmp".into()];
        p.sources.exclude_larger_than = Some(flux_core::ByteSize(1000));
        ProfileStore::new(&st.paths).save(&id, &p).unwrap();
        let snap = Service::open(st, &id, &p).unwrap().snapshots().unwrap()[0]
            .id
            .clone();

        std::fs::write(src.join("new.txt"), "n").unwrap();
        std::fs::create_dir_all(src.join("docs/extra")).unwrap();
        std::fs::write(src.join("docs/extra/inner.txt"), "i").unwrap();
        std::fs::write(src.join("keep.tmp"), "excluded").unwrap();
        std::fs::write(src.join("big.bin"), vec![0u8; 2000]).unwrap();
        std::fs::remove_file(src.join("b.txt")).unwrap();
        let all = vec![src.display().to_string()];

        let pv = crate::find::restore_preview_impl(
            st,
            "t".into(),
            snap.clone(),
            all.clone(),
            None,
            false,
            true,
        )
        .unwrap();
        assert_eq!(pv.remove, 2);
        assert_eq!(
            pv.remove_paths,
            vec![src.join("docs/extra"), src.join("new.txt")]
        );
        // Not exact: nothing would be removed.
        let pv = crate::find::restore_preview_impl(
            st,
            "t".into(),
            snap.clone(),
            all.clone(),
            None,
            false,
            false,
        )
        .unwrap();
        assert_eq!(pv.remove, 0);

        // Keeping files on restore (the default): the extras are moved aside.
        let r = crate::actions::restore_impl(
            st,
            "t".into(),
            snap.clone(),
            all.clone(),
            None,
            false,
            true,
            &|_| {},
        )
        .unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.removed, 2);
        assert!(src.join("b.txt").is_file(), "restored");
        assert!(!src.join("new.txt").exists() && !src.join("docs/extra").exists());
        assert_eq!(r.moved_aside.len(), 2);
        assert!(r.moved_aside.iter().all(|m| m.exists()));
        assert!(
            src.join("keep.tmp").exists(),
            "excluded: never backed up, left alone"
        );
        assert!(
            src.join("big.bin").exists(),
            "over the size limit: left alone"
        );

        // Again, overwriting: the kept copies are the person's and stay.
        let r = crate::actions::restore_impl(st, "t".into(), snap, all, None, true, true, &|_| {})
            .unwrap();
        assert_eq!(r.removed, 0);
        assert!(r.moved_aside.is_empty());
    }

    /// With cron chosen, a profile that backs up when its drive is plugged in hears that
    /// cron can't, and the preferences say which scheduler is in use. (Switching itself
    /// is tested through the CLI, where the crontab can be a fake.)
    #[test]
    fn cron_preference_and_on_connect() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path(), Scope::User);
        let st = Service::new(paths.clone());
        let mut p = Profile::new(
            "T",
            Target::Local {
                path: tmp.path().to_owned(),
                fs_uuid: Some("abcd".into()),
            },
            vec![tmp.path().join("src")],
        );
        p.schedule.on_connect = true;
        let warned = |st: &Service| {
            crate::edit::validate_profile_impl(st, serde_json::to_value(&p).unwrap())
                .unwrap()
                .iter()
                .any(|i| i.field == "schedule.on_connect" && i.severity == Severity::Warning)
        };
        assert!(!warned(&st));
        assert_eq!(
            crate::prefs::preferences_impl(&st).unwrap().scheduler,
            schedule::SchedulerKind::Systemd
        );
        let mut s = flux_runner::prefs::Settings::load(&paths);
        s.scheduler = schedule::SchedulerKind::Cron;
        s.save(&paths).unwrap();
        assert!(warned(&st));
        assert_eq!(
            crate::prefs::preferences_impl(&st).unwrap().scheduler,
            schedule::SchedulerKind::Cron
        );
        // Saving the other preferences keeps the choice.
        crate::prefs::save_preferences_impl(&st, true, None, None).unwrap();
        assert_eq!(
            flux_runner::prefs::Settings::load(&paths).scheduler,
            schedule::SchedulerKind::Cron
        );
    }

    /// A backup that can't start says why at once, instead of the app waiting on a job
    /// that's already over.
    #[test]
    fn backup_now_reports_a_start_that_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path(), Scope::User);
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let mut p = Profile::new(
            "T",
            Target::Local {
                path: dest,
                fs_uuid: None,
            },
            vec![tmp.path().join("src")],
        );
        p.options.notify = false;
        ProfileStore::new(&paths)
            .create(&ProfileId::new("t").unwrap(), &p)
            .unwrap();
        let with = |script: &str| {
            Service::with_commands(
                paths.clone(),
                vec!["sh".into(), "-c".into(), script.into(), "sh".into()],
                PathBuf::from("flux"),
            )
        };

        // Ended at once with an error: the error comes back.
        let e = backup_now_impl(
            &with("echo 'Error: the disk said no' >&2; exit 1"),
            "t".into(),
        )
        .unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("backup_failed", "the disk said no")
        );
        // Ended at once without one (nothing to do): not running.
        assert!(
            !backup_now_impl(&with("exit 0"), "t".into())
                .unwrap()
                .running
        );
        // Stop with nothing running says so in its own code.
        assert_eq!(
            cancel_impl(&with("true"), "t".into()).unwrap_err().code,
            "not_running"
        );

        // A drive that isn't there is refused before anything starts.
        let mut gone = p.clone();
        gone.target = Target::Local {
            path: PathBuf::from("/mnt/flux-test-no-such-drive"),
            fs_uuid: None,
        };
        ProfileStore::new(&paths)
            .create(&ProfileId::new("gone").unwrap(), &gone)
            .unwrap();
        let e = backup_now_impl(&with("exit 3"), "gone".into()).unwrap_err();
        assert_eq!(e.code, "not_connected");
        assert!(
            e.message
                .starts_with("flux-test-no-such-drive isn't connected"),
            "{}",
            e.message
        );
        let issues =
            crate::edit::validate_profile_impl(&with("true"), serde_json::to_value(&gone).unwrap())
                .unwrap();
        assert!(
            issues
                .iter()
                .any(|i| i.field == "target.connected" && i.severity == Severity::Warning),
            "{issues:?}"
        );
    }

    #[test]
    fn folder_index_matches_a_full_pass() {
        let mk = |p: &str, kind: Kind| Entry {
            path: p.as_bytes().to_vec(),
            kind,
            mode: 0o644,
            uid: 0,
            gid: 0,
            size: 1,
            mtime: 0,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            nlink: 1,
            rdev: 0,
            link: None,
            hash: None,
            xattrs: vec![],
        };
        let entries = vec![
            mk("home", Kind::Dir),
            mk("home/me", Kind::Dir),
            mk("home/me/a.txt", Kind::File),
            mk("home/me/docs", Kind::Dir),
            mk("home/me/docs/b.txt", Kind::File),
            mk("home/me/empty", Kind::Dir),
            mk("home/me/a.txt/odd", Kind::File), // parent isn't a folder
            mk("etc", Kind::Dir),
        ];
        let idx = SnapshotIndex::new(entries.clone());
        for folder in ["", "home", "home/me", "home/me/docs", "home/me/empty"] {
            let naive: Vec<u32> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| parent_of(&e.path) == folder.as_bytes())
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(idx.folders[folder.as_bytes()], naive, "{folder:?}");
        }
        assert!(
            !idx.folders.contains_key(&b"home/me/a.txt"[..]),
            "a file is no folder"
        );
        assert_eq!(idx.folders[&b"home/me/empty"[..]].len(), 0);
    }

    /// Setting the password while setting a backup up creates the encrypted Vault right
    /// away, so the first (scheduled) backup has nothing to ask; a wrong password for an
    /// existing Vault is refused.
    #[test]
    fn set_password_creates_the_vault_at_setup() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tgt")).unwrap();
        let paths = Paths::under(tmp.path().join("home"), flux_core::Scope::User);
        let key = tmp.path().join("keys/t.key");
        let mut p = Profile::new(
            "Test",
            Target::Local {
                path: tmp.path().join("tgt"),
                fs_uuid: None,
            },
            vec![tmp.path().to_path_buf()],
        );
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions {
            password_source: PasswordSource::File { path: key.clone() },
            ..VaultOptions::default()
        });
        let id = ProfileId::new("t").unwrap();
        ProfileStore::new(&paths).create(&id, &p).unwrap();
        let state = Service::new(paths.clone());

        set_password_impl(&state, "t".into(), "s3cret".into()).unwrap();
        // The key file didn't exist: it now holds the password, readable by its owner only.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::read_to_string(&key).unwrap().trim(), "s3cret");
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // The Vault exists and opens with no one asked (the key file is the source).
        assert!(flux_runner::repo::open_vault(&paths, &id, &p, false, None).is_ok());
        assert!(!needs_password(&state, &id, &p));
        // A wrong password for the existing Vault is refused.
        let e = set_password_impl(&state, "t".into(), "wrong".into()).unwrap_err();
        assert_eq!(e.code(), "wrong_password");
        assert!(set_password_impl(&state, "t".into(), "s3cret".into()).is_ok());
    }

    #[test]
    fn mirror_views() {
        let e = env(Engine::Mirror);
        let list = profiles_impl(&e.state).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].last_run.as_ref().unwrap().status,
            flux_runner::RunStatus::Completed
        );

        let o = overview_impl(&e.state, "t".into()).unwrap();
        assert_eq!(o.snapshot_count, 1);
        assert!(o.repo_error.is_none());
        assert!(o.free_bytes.is_some());

        let snap = snapshots_impl(&e.state, "t".into()).unwrap().remove(0);
        let root = list_dir_impl(&e.state, "t".into(), snap.id.clone(), "/".into()).unwrap();
        assert_eq!(root.entries.len(), 1, "only the top-level ancestor");
        let src = e.src.display().to_string();
        let l = list_dir_impl(&e.state, "t".into(), snap.id.clone(), src.clone()).unwrap();
        let names: Vec<_> = l
            .entries
            .iter()
            .map(|x| (x.name.as_str(), x.children))
            .collect();
        assert_eq!(names, [("b.txt", None), ("docs", Some(1))]);
        assert!(l.entries.iter().all(|x| x.on_disk == Some(true)));
        // Deleted since the backup: marked, so it's easy to find and bring back.
        std::fs::remove_file(e.src.join("b.txt")).unwrap();
        let l = list_dir_impl(&e.state, "t".into(), snap.id.clone(), src.clone()).unwrap();
        let gone: Vec<_> = l
            .entries
            .iter()
            .map(|x| (x.name.as_str(), x.on_disk))
            .collect();
        assert_eq!(gone, [("b.txt", Some(false)), ("docs", Some(true))]);
        let found = crate::find::find_impl(&e.state, "t".into(), "b.txt".into()).unwrap();
        assert_eq!(found.hits[0].on_disk, Some(false), "search says so too");
        // Restored: the next look at the folder (and the next search) shows it back.
        crate::actions::restore_impl(
            &e.state,
            "t".into(),
            snap.id.clone(),
            vec![format!("{src}/b.txt")],
            None,
            false,
            false,
            &|_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(e.src.join("b.txt")).unwrap(), "beta");
        let l = list_dir_impl(&e.state, "t".into(), snap.id.clone(), src.clone()).unwrap();
        assert!(l.entries.iter().all(|x| x.on_disk == Some(true)));
        let found = crate::find::find_impl(&e.state, "t".into(), "b.txt".into()).unwrap();
        assert_eq!(found.hits[0].on_disk, Some(true));
        assert!(
            list_dir_impl(
                &e.state,
                "t".into(),
                snap.id.clone(),
                format!("{src}/b.txt")
            )
            .is_err(),
            "files aren't folders"
        );

        let out = e._tmp.path().join("restored");
        let r = crate::actions::restore_impl(
            &e.state,
            "t".into(),
            snap.id,
            vec![format!("{src}/docs")],
            Some(out.display().to_string()),
            false,
            false,
            &|_| {},
        )
        .unwrap();
        assert!(r.errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(out.join("docs/a.txt")).unwrap(),
            "alpha"
        );
    }

    #[test]
    fn finding_files() {
        let e = env(Engine::Mirror);
        // A second snapshot where b.txt changed.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(e.src.join("b.txt"), "beta, longer").unwrap();
        let (id, p) = e.state.load("t").unwrap();
        let r = run_backup(
            &BackupJob {
                paths: &e.state.paths,
                id: &id,
                profile: &p,
                name: None,
                dry_run: false,
                trigger: Trigger::Manual,
                prune: false,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        );
        assert!(r.outcome.is_ok(), "{:?}", r.outcome.err());
        assert!(
            r.record.free_after.is_some(),
            "free space is recorded for the forecast"
        );
        let snaps = snapshots_impl(&e.state, "t".into()).unwrap();
        assert_eq!(snaps.len(), 2);

        let found = crate::find::find_impl(&e.state, "t".into(), "B.TXT".into()).unwrap();
        assert_eq!(found.snapshots, 2);
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].found.versions, 2);
        assert_eq!(found.hits[0].found.latest_size, 12);
        let path = format!("{}/b.txt", e.src.display());
        assert_eq!(found.hits[0].found.path, path);
        assert_eq!(found.hits[0].on_disk, Some(true));

        // The estimate follows the exclusions.
        let mut prof = serde_json::to_value(&p).unwrap();
        let est = crate::space::estimate_impl(&e.state, prof.clone()).unwrap();
        assert_eq!((est.files, est.bytes), (2, 5 + 12));
        assert!(est.free.is_some());
        prof["sources"]["exclude"] = serde_json::json!(["docs/"]);
        let est = crate::space::estimate_impl(&e.state, prof).unwrap();
        assert_eq!((est.files, est.bytes), (1, 12));

        let v = crate::find::versions_impl(&e.state, "t".into(), path.clone()).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].size, v[1].size), (4, 12));

        // Restoring the first version in place: b.txt differs, docs/a.txt is identical.
        let pv = crate::find::restore_preview_impl(
            &e.state,
            "t".into(),
            snaps[0].id.clone(),
            vec![e.src.display().to_string()],
            None,
            false,
            false,
        )
        .unwrap();
        assert_eq!((pv.new, pv.differ, pv.identical), (0, 1, 1));
        assert_eq!(pv.differ_paths, vec![e.src.join("b.txt")]);
        assert!(pv.keep_existing);
        assert_eq!(
            std::fs::read_to_string(e.src.join("b.txt")).unwrap(),
            "beta, longer",
            "a preview changes nothing"
        );
    }

    #[test]
    fn search_uses_an_index() {
        let e = env(Engine::Rsync);
        let idx = e.state.paths.cache_dir.join("index/t");
        assert!(!idx.exists());
        let found = crate::find::find_impl(&e.state, "t".into(), "a.txt".into()).unwrap();
        assert_eq!(found.hits.len(), 1);
        let files: Vec<_> = std::fs::read_dir(&idx).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "one index file per snapshot");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            files[0].metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Searching again reads the index: the same answer.
        let again = crate::find::find_impl(&e.state, "t".into(), "a.txt".into()).unwrap();
        assert_eq!(again.hits[0].found.path, found.hits[0].found.path);
        // A file for a snapshot that's gone is dropped.
        std::fs::write(idx.join("gone-1.idx"), b"x").unwrap();
        crate::find::versions_impl(&e.state, "t".into(), found.hits[0].found.path.clone()).unwrap();
        assert!(!idx.join("gone-1.idx").exists());
        // Deleting the profile removes its index.
        crate::edit::delete_profile_impl(&e.state, "t".into()).unwrap();
        assert!(!idx.exists());
    }

    #[test]
    fn vault_lock_and_unlock() {
        let e = env(Engine::Vault);
        let err = snapshots_impl(&e.state, "t".into()).unwrap_err();
        assert_eq!(err.code, "password_required");
        assert_eq!(
            overview_impl(&e.state, "t".into()).unwrap_err().code,
            "password_required"
        );
        assert_eq!(
            unlock_impl(&e.state, "t".into(), "wrong".into(), false)
                .unwrap_err()
                .code,
            "wrong_password"
        );
        unlock_impl(&e.state, "t".into(), "pw".into(), false).unwrap();
        let snaps = snapshots_impl(&e.state, "t".into()).unwrap();
        assert_eq!(snaps.len(), 1);
        let l = list_dir_impl(
            &e.state,
            "t".into(),
            snaps[0].id.clone(),
            e.src.display().to_string(),
        )
        .unwrap();
        assert_eq!(l.entries.len(), 2);
        // With the password in memory, a background backup can be started without asking.
        assert!(!needs_password(
            &e.state,
            &ProfileId::new("t").unwrap(),
            &e.state.load("t").unwrap().1
        ));
    }

    #[test]
    fn new_vaults_and_key_files() {
        let e = env(Engine::Mirror);
        let st = &e.state;
        let mut p = st.load("t").unwrap().1;
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions {
            password_source: PasswordSource::Prompt,
            ..VaultOptions::default()
        });
        p.target = Target::Local {
            path: e._tmp.path().join("fresh"),
            fs_uuid: None,
        };
        std::fs::create_dir_all(e._tmp.path().join("fresh")).unwrap();
        let id = ProfileId::new("fresh").unwrap();
        ProfileStore::new(&st.paths).create(&id, &p).unwrap();
        // No Vault there yet: the app is told to ask for a new password, then accepts it.
        assert_eq!(
            unlock_impl(st, "fresh".into(), "s3cret".into(), false)
                .unwrap_err()
                .code,
            "new_vault"
        );
        unlock_impl(st, "fresh".into(), "s3cret".into(), true).unwrap();
        assert!(!needs_password(st, &id, &p));

        let path = crate::edit::save_key_file_impl(st, "fresh".into()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret\n");
        let saved = st.load("fresh").unwrap().1;
        assert!(matches!(
            saved.vault.unwrap().password_source,
            PasswordSource::File { path: ref p } if p == std::path::Path::new(&path)
        ));
        // Mirrors have no password to save.
        assert!(crate::edit::save_key_file_impl(st, "t".into()).is_err());
    }

    #[test]
    fn editing_and_actions() {
        use crate::{actions, edit};
        let e = env(Engine::Mirror);
        let st = &e.state;
        // Validate and save: a broken edit is refused with field-addressed issues.
        let mut p = serde_json::to_value(edit::get_profile_impl(st, "t".into()).unwrap()).unwrap()
            ["profile"]
            .clone();
        p["sources"]["paths"] = serde_json::json!([]);
        let r = edit::save_profile_impl(st, "t".into(), p.clone(), false).unwrap();
        assert!(!r.saved_flag());
        assert!(
            edit::validate_profile_impl(st, p.clone())
                .unwrap()
                .iter()
                .any(|i| i.field == "sources.paths")
        );
        p["sources"]["paths"] = serde_json::json!([e.src]);
        p["name"] = "Renamed".into();
        assert!(
            edit::save_profile_impl(st, "t".into(), p.clone(), false)
                .unwrap()
                .saved_flag()
        );
        assert_eq!(profiles_impl(st).unwrap()[0].name, "Renamed");
        // A typo'd key is a parse error, not silently ignored.
        let mut bad = p.clone();
        bad["sources"]["exclud"] = serde_json::json!(["x"]);
        assert!(!edit::validate_profile_impl(st, bad).unwrap().is_empty());

        // New profiles get a free id and are created once.
        assert_eq!(edit::suggest_id_impl(st, "T".into()).unwrap(), "t-2");
        assert!(
            edit::save_profile_impl(st, "copy".into(), p.clone(), true)
                .unwrap()
                .saved_flag()
        );
        assert!(
            edit::save_profile_impl(st, "copy".into(), p, true).is_err(),
            "create refuses an existing id"
        );
        edit::delete_profile_impl(st, "copy".into()).unwrap();

        // Retention preview: keep-last 0 + nothing else still keeps the newest.
        let policy = serde_json::json!({"keep_last": 0, "keep_daily": 0, "keep_weekly": 0, "keep_monthly": 0, "keep_yearly": 0});
        let rows = edit::retention_preview_impl(st, "t".into(), policy).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].keep_flag());

        // Snapshot actions.
        let snap = snapshots_impl(st, "t".into()).unwrap().remove(0);
        let m = actions::update_snapshot_impl(
            st,
            "t".into(),
            snap.id.clone(),
            Some("milestone".into()),
            true,
            Some(true),
        )
        .unwrap();
        assert_eq!((m.name.as_deref(), m.pinned), (Some("milestone"), true));
        std::fs::write(e.src.join("c.txt"), "new").unwrap();
        let c = actions::compare_impl(st, "t".into(), snap.id.clone(), None).unwrap();
        assert_eq!(c.added(), 1);
        assert_eq!(c.left_out_by_size(), 0);
        // With a size limit, a big new file isn't "added" (no backup will hold it) but
        // counted; big files the snapshot already holds are still compared.
        std::fs::write(e.src.join("big.bin"), vec![1u8; 4096]).unwrap();
        let store = ProfileStore::new(&st.paths);
        let id = ProfileId::new("t").unwrap();
        let mut p = store.load(&id).unwrap();
        p.sources.exclude_larger_than = Some(flux_core::ByteSize(3));
        store.save(&id, &p).unwrap();
        let c = actions::compare_impl(st, "t".into(), snap.id.clone(), None).unwrap();
        assert_eq!(c.left_out_by_size(), 1, "big.bin");
        assert_eq!(
            c.added(),
            1,
            "c.txt (3 bytes) is within the limit; big.bin isn't"
        );
        assert_eq!(
            c.removed(),
            0,
            "a.txt and b.txt are in the snapshot: still compared"
        );
        assert_eq!(actions::runs_impl(st, "t".into(), 10).unwrap().len(), 1);
    }

    #[test]
    fn compatibility_mode_views() {
        if std::process::Command::new("rsync")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let e = env(Engine::Rsync);
        // Browsing works on the plain folders.
        let snaps = snapshots_impl(&e.state, "t".into()).unwrap();
        assert_eq!(snaps.len(), 1);
        let dir = e.src.join("docs").display().to_string();
        let l = list_dir_impl(&e.state, "t".into(), snaps[0].id.clone(), dir).unwrap();
        assert_eq!(l.entries.len(), 1);
        // The recovery kit, and the tools.
        let kit = recovery_kit_impl(&e.state, "t".into()).unwrap();
        assert_eq!(kit.engine, Engine::Rsync);
        assert!(kit.recovery.contains("RESTORE ONE FILE"));
        assert!(kit.script.starts_with("#!/bin/sh"));
        assert!(kit.location.contains("flux-rsync"));
        let tools = tools_impl();
        assert_eq!(tools.len(), 2);
        assert!(tools[0].supported);
        // rsync has no password; an encrypted Borg profile needs one.
        let (id, mut p) = e.state.load("t").unwrap();
        assert!(!needs_password(&e.state, &id, &p));
        p.engine = Engine::Borg;
        p.borg = Some(flux_core::profile::BorgOptions {
            password_source: PasswordSource::Prompt,
            ..Default::default()
        });
        assert!(needs_password(&e.state, &id, &p));
        p.borg.as_mut().unwrap().encryption = flux_core::profile::BorgEncryption::None;
        assert!(!needs_password(&e.state, &id, &p));
        // Other engines have no kit.
        let m = env(Engine::Mirror);
        assert!(recovery_kit_impl(&m.state, "t".into()).is_err());
    }
}
