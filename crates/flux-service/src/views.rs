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
use flux_runner::schedule::{self, Systemd};
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
    schedule: String,
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
        schedule: schedule::summary(p),
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
                schedule: String::new(),
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
    next_run: Option<String>,
    timer_active: bool,
    free_bytes: Option<u64>,
    /// Why snapshots couldn't be read (e.g. the drive isn't connected).
    repo_error: Option<String>,
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
    let st = schedule::status(&Systemd::new(&state.paths), &id, &p, &state.unit_env());
    let free_bytes = match &p.target {
        Target::Local { path, .. } => rustix::fs::statvfs(path)
            .ok()
            .map(|v| v.f_bavail.saturating_mul(v.f_frsize)),
        Target::Ssh { .. } => None,
    };
    Ok(Overview {
        profile: summary(&state.paths, &id, &p, &running),
        snapshot_count: snaps.len(),
        oldest: snaps.first().map(|s| s.started),
        latest: snaps.last().cloned(),
        recent_runs: runlog::recent_runs(&state.paths, &id, 8),
        next_run: st.next_run,
        timer_active: st.timer_active,
        free_bytes,
        repo_error,
    })
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

pub fn list_dir_impl(
    state: &Service,
    id: String,
    snapshot: String,
    path: String,
) -> Result<Listing> {
    let key = (id.clone(), snapshot.clone());
    let cached = state
        .entries
        .lock()
        .expect("cache")
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.clone());
    let all = match cached {
        Some(v) => v,
        None => {
            let (pid, p) = state.load(&id)?;
            let repo = state.open(&pid, &p)?;
            let snap = repo.find(&snapshot)?;
            let v: Vec<Entry> = repo
                .entries(&snap)?
                .collect::<std::result::Result<_, _>>()?;
            let v = Arc::new(v);
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
    if !want.is_empty() && !all.iter().any(|e| e.path == want && e.kind == Kind::Dir) {
        return Err(ApiError::new(
            "error",
            format!("{path} is not a folder in this snapshot"),
        ));
    }
    let mut children: HashMap<&[u8], usize> = HashMap::new();
    for e in all.iter() {
        *children.entry(parent_of(&e.path)).or_default() += 1;
    }
    let entries = all
        .iter()
        .filter(|e| parent_of(&e.path) == want.as_slice())
        .map(|e| DirEntry {
            name: String::from_utf8_lossy(
                &e.path[parent_of(&e.path).len() + usize::from(!want.is_empty())..],
            )
            .into_owned(),
            path: format!("/{}", String::from_utf8_lossy(&e.path)),
            kind: e.kind,
            size: e.size,
            mtime: e.mtime,
            mode: e.mode,
            link: e
                .link
                .as_ref()
                .map(|l| String::from_utf8_lossy(l).into_owned()),
            children: (e.kind == Kind::Dir)
                .then(|| children.get(e.path.as_slice()).copied().unwrap_or(0)),
        })
        .collect();
    Ok(Listing { path, entries })
}

/// Check a Vault password and keep it in memory. If the Vault doesn't exist yet, the
/// password becomes the new Vault's when `create` is set (the first backup creates it);
/// otherwise the answer is `new_vault`, so the app can ask for it twice.
pub fn unlock_impl(state: &Service, id: String, password: String, create: bool) -> Result<()> {
    let password = Zeroizing::new(password);
    let (pid, p) = state.load(&id)?;
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

/// Would a background `flux backup` of this profile be unable to get its password?
pub fn needs_password(state: &Service, id: &ProfileId, p: &Profile) -> bool {
    if p.engine != Engine::Vault
        || p.vault.as_ref().is_some_and(|v| !v.encryption)
        || std::env::var_os("FLUX_PASSWORD").is_some()
    {
        return false;
    }
    if state.password(id).is_some() {
        return false;
    }
    match p
        .vault
        .as_ref()
        .map(|v| &v.password_source)
        .unwrap_or(&PasswordSource::Keyring)
    {
        // Root has no desktop keyring to look in.
        PasswordSource::Keyring if state.scope() == Scope::System => true,
        PasswordSource::Keyring => !matches!(password::keyring_lookup(id), Ok(Some(_))),
        PasswordSource::Prompt => true,
        PasswordSource::File { .. } | PasswordSource::Command { .. } => false,
    }
}

/// Start a backup as a separate `flux backup` process, so it survives closing the window
/// and shows up for every client (GUI, `flux watch`) through its status socket.
pub fn backup_now_impl(state: &Service, id: String) -> Result<()> {
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
    cmd.args(args)
        .arg(pid.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(pw) = state.password(&pid) {
        cmd.env("FLUX_PASSWORD", pw.as_str());
    }
    let mut child = cmd.spawn().map_err(|e| {
        ApiError::new(
            "error",
            format!("cannot start {}: {e}", program.to_string_lossy()),
        )
    })?;
    // Reap it in the background; the job's own socket reports progress and the outcome.
    let stderr = child.stderr.take();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        let status = child.wait();
        if !matches!(status, Ok(s) if s.success()) {
            tracing::warn!("flux backup exited with {status:?}: {}", text.trim());
        }
    });
    Ok(())
}

pub fn cancel_impl(state: &Service, id: String) -> Result<()> {
    let id = ProfileId::new(id)?;
    jobs::Client::connect(&state.paths, &id)?.cancel()?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        assert_eq!(actions::runs_impl(st, "t".into(), 10).unwrap().len(), 1);
    }
}
