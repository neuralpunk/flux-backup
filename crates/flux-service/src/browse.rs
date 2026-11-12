//! *Browse in Files*: one snapshot mounted read-only (with `flux mount`, FUSE) in a folder
//! of the runtime directory, for the desktop's file manager to open. One at a time: a new
//! one, *Close*, or the app quitting unmounts the last.
//!
//! The mount runs as its own process, the same `flux` the app runs backups with; it
//! unmounts on SIGINT. A mount left behind by an app that didn't quit cleanly is unmounted
//! (lazily) the next time.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use flux_core::{Scope, sys};

use flux_core::sync::Locked;

use crate::{ApiError, Result, Service};

/// How long a mount may take to appear (a Vault reads its index first).
const MOUNT_WAIT: Duration = Duration::from_secs(30);

/// A snapshot open in the file manager.
pub struct Mounted {
    profile: String,
    snapshot: String,
    dir: PathBuf,
    child: Child,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Browsing {
    pub profile: String,
    pub snapshot: String,
    /// Where it's mounted.
    pub dir: PathBuf,
    /// The folder to open: the snapshot's only source inside the mount, or the mount.
    pub open: PathBuf,
}

fn browse_root(state: &Service) -> PathBuf {
    state.paths.runtime_dir.join("browse")
}

/// A folder name for one snapshot's mount (ids are safe already; be sure).
fn dir_name(profile: &str, snapshot: &str) -> String {
    format!("{profile}@{snapshot}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_@.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The backup that takes `path` (a file manager's *Previous versions…*): the one whose
/// folder holds it most closely. `None` when none of this scope's backups takes it.
pub fn backup_for_path_impl(state: &Service, path: String) -> Result<Option<String>> {
    let path = PathBuf::from(&path);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let store = flux_core::ProfileStore::new(&state.paths);
    let mut best: Option<(usize, String)> = None;
    for listed in store.list().unwrap_or_default() {
        let (id, Ok(p)) = (listed.id, listed.profile) else {
            continue;
        };
        for src in &p.sources.paths {
            let src = std::fs::canonicalize(src).unwrap_or_else(|_| src.clone());
            let depth = src.components().count();
            if path.starts_with(&src) && best.as_ref().is_none_or(|(d, _)| depth > *d) {
                best = Some((depth, id.to_string()));
            }
        }
    }
    Ok(best.map(|(_, id)| id))
}

/// Mount `snapshot` of backup `id` and say where (the app then opens it).
pub fn browse_in_files_impl(state: &Service, id: String, snapshot: String) -> Result<Browsing> {
    if state.scope() == Scope::System {
        return Err(ApiError::new(
            "unsupported",
            "Browse in Files isn't available for system backups (their files belong to root): browse them here in Flux.",
        ));
    }
    let (pid, p) = state.load(&id)?;
    let meta = state.open(&pid, &p)?.find(&snapshot)?;
    {
        let mut cur = state.browsing.locked();
        if let Some(m) = cur.as_mut()
            && m.profile == pid.as_str()
            && m.snapshot == meta.id
            && matches!(m.child.try_wait(), Ok(None))
            && sys::is_mount_point(&m.dir)
        {
            return Ok(view(m, &meta.sources));
        }
        if let Some(old) = cur.take() {
            stop(old);
        }
    }
    let root = browse_root(state);
    let dir = root.join(dir_name(pid.as_str(), &meta.id));
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::new("error", format!("{}: {e}", dir.display())))?;
    if sys::is_mount_point(&dir) {
        // Left by an app that didn't quit cleanly.
        lazy_unmount(&dir);
    }
    let log = root.join(format!("{}.log", dir_name(pid.as_str(), &meta.id)));
    let said = std::fs::File::create(&log)
        .map_err(|e| ApiError::new("error", format!("{}: {e}", log.display())))?;
    let mut cmd = Command::new(state.flux_program());
    cmd.args(["mount", pid.as_str()])
        .arg(&dir)
        .args(["--snapshot", &meta.id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(said))
        .process_group(0);
    if let Some(pw) = state.password(&pid) {
        cmd.env("FLUX_PASSWORD", pw.as_str());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::new("error", format!("cannot run flux mount: {e}")))?;
    let deadline = Instant::now() + MOUNT_WAIT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let why = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .trim_start_matches("error: ")
                .to_owned();
            return Err(ApiError::new(
                "error",
                if why.is_empty() {
                    format!("couldn't open the snapshot in Files ({status})")
                } else {
                    format!("couldn't open the snapshot in Files: {why}")
                },
            ));
        }
        if sys::is_mount_point(&dir) {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ApiError::new(
                "error",
                "couldn't open the snapshot in Files: mounting it took too long",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let m = Mounted {
        profile: pid.to_string(),
        snapshot: meta.id.clone(),
        dir,
        child,
    };
    let v = view(&m, &meta.sources);
    *state.browsing.locked() = Some(m);
    Ok(v)
}

fn view(m: &Mounted, sources: &[PathBuf]) -> Browsing {
    // One source: open it, not the empty folders above it.
    let open = match sources {
        [one] => {
            let inside = m.dir.join(one.strip_prefix("/").unwrap_or(one));
            if inside.is_dir() {
                inside
            } else {
                m.dir.clone()
            }
        }
        _ => m.dir.clone(),
    };
    Browsing {
        profile: m.profile.clone(),
        snapshot: m.snapshot.clone(),
        dir: m.dir.clone(),
        open,
    }
}

/// What's open in the file manager, if anything.
pub fn browsing_impl(state: &Service) -> Result<Option<Browsing>> {
    let mut cur = state.browsing.locked();
    let alive = cur
        .as_mut()
        .is_some_and(|m| matches!(m.child.try_wait(), Ok(None)) && sys::is_mount_point(&m.dir));
    if !alive {
        // Unmounted from outside (or the mount stopped): forget it.
        if let Some(old) = cur.take() {
            stop(old);
        }
        return Ok(None);
    }
    Ok(cur.as_ref().map(|m| Browsing {
        profile: m.profile.clone(),
        snapshot: m.snapshot.clone(),
        dir: m.dir.clone(),
        open: m.dir.clone(),
    }))
}

/// Unmount what's open in the file manager.
pub fn stop_browsing_impl(state: &Service) -> Result<()> {
    if let Some(m) = state.browsing.locked().take() {
        stop(m);
    }
    Ok(())
}

/// Ask the mount to stop (it unmounts on SIGINT), then make sure.
fn stop(mut m: Mounted) {
    if let Some(pid) = rustix::process::Pid::from_raw(m.child.id() as i32) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::INT);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = m.child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if matches!(m.child.try_wait(), Ok(None)) {
        // Busy (a file manager window still inside): detach it, then end the process.
        lazy_unmount(&m.dir);
        let _ = m.child.kill();
        let _ = m.child.wait();
    }
    if sys::is_mount_point(&m.dir) {
        lazy_unmount(&m.dir);
    }
    let _ = std::fs::remove_dir(&m.dir);
}

fn lazy_unmount(dir: &Path) {
    let _ = Command::new("fusermount3")
        .args(["-u", "-z"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// For real: a backup, its snapshot mounted for the file manager, a file read through
    /// it, another snapshot replacing it, and Close unmounting. Needs FUSE and the `flux`
    /// binary built next to the tests (skipped otherwise).
    #[test]
    fn a_snapshot_opens_in_files_and_closes() {
        use flux_core::profile::Target;
        use flux_core::{Paths, ProfileId, ProfileStore};
        let flux = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/flux");
        if !flux.exists() || !Path::new("/dev/fuse").exists() || which("fusermount3").is_none() {
            eprintln!("skipped: needs target/debug/flux and FUSE");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        std::fs::create_dir_all(t.join("src")).unwrap();
        std::fs::create_dir_all(t.join("tgt")).unwrap();
        std::fs::write(t.join("src/note.txt"), "first").unwrap();
        let root = t.join("flux");
        let paths = Paths::under(&root, Scope::User);
        // The mount runs as `flux`, told to use the same folders.
        let wrapper = t.join("flux.sh");
        // Written elsewhere, then made executable by `install` (a process of its own), so
        // no child of this multithreaded test process holds it open: "Text file busy".
        let text = t.join("flux.sh.txt");
        std::fs::write(
            &text,
            format!(
                "#!/bin/sh\nFLUX_HOME='{}' exec '{}' \"$@\"\n",
                root.display(),
                flux.display()
            ),
        )
        .unwrap();
        assert!(
            Command::new("install")
                .args(["-m", "0755"])
                .arg(&text)
                .arg(&wrapper)
                .status()
                .unwrap()
                .success()
        );
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
        let backup = || {
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
            )
            .record
            .snapshot
            .unwrap()
        };
        let first = backup();
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(t.join("src/note.txt"), "second").unwrap();
        let second = backup();
        let state = Service::with_commands(
            paths.clone(),
            vec![wrapper.clone().into_os_string(), "backup".into()],
            wrapper,
        );

        let b = browse_in_files_impl(&state, "p".into(), first.clone()).unwrap();
        assert!(sys::is_mount_point(&b.dir), "{b:?}");
        assert_eq!(b.open, b.dir.join(t.join("src").strip_prefix("/").unwrap()));
        assert_eq!(
            std::fs::read_to_string(b.open.join("note.txt")).unwrap(),
            "first"
        );
        assert!(
            std::fs::write(b.open.join("new.txt"), "x").is_err(),
            "read-only"
        );
        assert_eq!(
            browsing_impl(&state).unwrap().map(|x| x.snapshot),
            Some(first)
        );

        // Another snapshot: the first is unmounted, the second opened.
        let c = browse_in_files_impl(&state, "p".into(), second.clone()).unwrap();
        assert!(!sys::is_mount_point(&b.dir) && !b.dir.exists());
        assert_eq!(
            std::fs::read_to_string(c.open.join("note.txt")).unwrap(),
            "second"
        );

        stop_browsing_impl(&state).unwrap();
        assert!(!sys::is_mount_point(&c.dir) && !c.dir.exists());
        assert_eq!(browsing_impl(&state).unwrap(), None);
        // An unknown snapshot says so, and mounts nothing.
        assert!(browse_in_files_impl(&state, "p".into(), "nope".into()).is_err());
    }

    fn which(program: &str) -> Option<PathBuf> {
        std::env::var_os("PATH")?
            .to_str()?
            .split(':')
            .map(|d| Path::new(d).join(program))
            .find(|p| p.exists())
    }

    #[test]
    fn the_backup_that_takes_a_path_is_the_closest() {
        use flux_core::profile::Target;
        use flux_core::{Paths, ProfileId, ProfileStore};
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        for d in ["home/docs/work", "other"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        let paths = Paths::under(t.join("flux"), Scope::User);
        let store = ProfileStore::new(&paths);
        for (id, src) in [("home", "home"), ("work", "home/docs/work")] {
            let p = flux_core::Profile::new(
                id,
                Target::Local {
                    path: t.join("tgt"),
                    fs_uuid: None,
                },
                vec![t.join(src)],
            );
            store.create(&ProfileId::new(id).unwrap(), &p).unwrap();
        }
        let state = Service::new(paths);
        let of = |p: &str| backup_for_path_impl(&state, t.join(p).display().to_string()).unwrap();
        assert_eq!(of("home/docs/a.txt").as_deref(), Some("home"));
        assert_eq!(of("home/docs/work/b.txt").as_deref(), Some("work"));
        assert_eq!(of("home/docs/work").as_deref(), Some("work"));
        assert_eq!(of("other"), None);
    }

    #[test]
    fn mount_folders_are_safe_names() {
        assert_eq!(
            dir_name("home", "2026-11-10_20-00-00"),
            "home@2026-11-10_20-00-00"
        );
        assert_eq!(dir_name("a/b", "../x y"), "a_b@.._x_y");
    }
}
