//! On-disk layout of a Mirror repository:
//!
//! ```text
//! <target>/flux-backup/<host>/<user>/<profile>/
//! ├── flux-repo.toml          format marker
//! ├── lock                    flock(2) target; held while a job writes
//! ├── latest -> snapshots/<id>
//! └── snapshots/
//!     ├── .partial/           snapshot under construction (resumable)
//!     └── 2026-11-04_18-30-00/
//!         ├── meta.json       what, when, how it went
//!         ├── manifest.zst    metadata of every path
//!         └── data/           the files, at their original absolute paths
//! ```

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MirrorError;

pub const REPO_FORMAT: u32 = 1;
pub const REPO_MARKER: &str = "flux-repo.toml";
pub const PARTIAL: &str = ".partial";
const META: &str = "meta.json";
const MANIFEST: &str = "manifest.zst";
const DATA: &str = "data";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub format: u32,
    pub engine: String,
    pub profile: String,
    pub host: String,
    pub user: String,
    pub created: Timestamp,
}

pub use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus};

/// An open Mirror repository.
#[derive(Debug, Clone)]
pub struct MirrorRepo {
    root: PathBuf,
}

/// Exclusive lock on a repository; released on drop.
#[derive(Debug)]
pub struct RepoLock {
    _file: File,
}

impl MirrorRepo {
    /// Repository root for a profile on a local target.
    pub fn locate(target: &Path, host: &str, user: &str, profile: &str) -> PathBuf {
        target
            .join("flux-backup")
            .join(host)
            .join(user)
            .join(profile)
    }

    pub fn open(root: &Path) -> Result<Self, MirrorError> {
        let marker = root.join(REPO_MARKER);
        let text = match fs::read_to_string(&marker) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(MirrorError::NoRepository(root.to_owned()));
            }
            Err(e) => return Err(MirrorError::io(&marker, e)),
        };
        let info: RepoInfo = toml::from_str(&text)
            .map_err(|e| MirrorError::Corrupt(format!("{}: {e}", marker.display())))?;
        if info.engine != "mirror" {
            return Err(MirrorError::Corrupt(format!(
                "{} is a {} repository, not a mirror repository",
                root.display(),
                info.engine
            )));
        }
        if info.format > REPO_FORMAT {
            return Err(MirrorError::Corrupt(format!(
                "{} uses repository format {}; this Flux supports up to {REPO_FORMAT}",
                root.display(),
                info.format
            )));
        }
        Ok(Self {
            root: root.to_owned(),
        })
    }

    pub fn init(root: &Path, profile: &str, host: &str, user: &str) -> Result<Self, MirrorError> {
        fs::create_dir_all(root.join("snapshots")).map_err(|e| MirrorError::io(root, e))?;
        // Manifests list every file name, including names inside private directories.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|e| MirrorError::io(root, e))?;
        let info = RepoInfo {
            format: REPO_FORMAT,
            engine: "mirror".into(),
            profile: profile.into(),
            host: host.into(),
            user: user.into(),
            created: Timestamp::now(),
        };
        let text =
            toml::to_string_pretty(&info).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
        flux_core::store::write_atomic(&root.join(REPO_MARKER), text.as_bytes(), 0o644)?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    pub fn open_or_init(
        root: &Path,
        profile: &str,
        host: &str,
        user: &str,
    ) -> Result<Self, MirrorError> {
        match Self::open(root) {
            Err(MirrorError::NoRepository(_)) => Self::init(root, profile, host, user),
            r => r,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Take the repository lock without waiting.
    pub fn lock(&self) -> Result<RepoLock, MirrorError> {
        let path = self.root.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o644)
            .open(&path)
            .map_err(|e| MirrorError::io(&path, e))?;
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                self.cleanup_deleting();
                Ok(RepoLock { _file: file })
            }
            Err(rustix::io::Errno::WOULDBLOCK) => Err(MirrorError::Locked(self.root.clone())),
            Err(e) => Err(MirrorError::io(&path, e.into())),
        }
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    pub fn snapshot_dir(&self, id: &str) -> PathBuf {
        self.snapshots_dir().join(id)
    }

    pub fn partial_dir(&self) -> PathBuf {
        self.snapshots_dir().join(PARTIAL)
    }

    pub fn data_dir(&self, id: &str) -> PathBuf {
        snapshot_data(&self.snapshot_dir(id))
    }

    pub fn manifest_path(&self, id: &str) -> PathBuf {
        snapshot_manifest(&self.snapshot_dir(id))
    }

    /// All completed snapshots, oldest first. Unreadable entries are skipped with a log
    /// warning so one damaged snapshot doesn't hide the rest.
    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, MirrorError> {
        let dir = self.snapshots_dir();
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(MirrorError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for ent in rd {
            let ent = ent.map_err(|e| MirrorError::io(&dir, e))?;
            let name = ent.file_name();
            if name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            match read_meta(&ent.path()) {
                Ok(m) => out.push(m),
                Err(e) => tracing::warn!("skipping snapshot {}: {e}", ent.path().display()),
            }
        }
        out.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Change a snapshot's `meta.json` (name, pin). Caller should hold the lock.
    pub fn update_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut SnapshotMeta),
    ) -> Result<SnapshotMeta, MirrorError> {
        let dir = self.snapshot_dir(id);
        let mut meta = read_meta(&dir).map_err(|_| MirrorError::SnapshotNotFound(id.into()))?;
        f(&mut meta);
        write_meta(&dir, &meta)?;
        Ok(meta)
    }

    /// Delete a snapshot. It is first renamed to `.deleting-<id>` so it disappears from
    /// listings atomically. A crash mid-delete leaves a dot-directory that the next lock
    /// holder removes. Caller must hold the lock.
    pub fn delete_snapshot(&self, id: &str) -> Result<(), MirrorError> {
        if id.is_empty() || id.starts_with('.') || id.contains('/') {
            return Err(MirrorError::SnapshotNotFound(id.into()));
        }
        let dir = self.snapshot_dir(id);
        let doomed = self.snapshots_dir().join(format!(".deleting-{id}"));
        fs::rename(&dir, &doomed).map_err(|e| MirrorError::io(&dir, e))?;
        flux_fs::fsops::remove_force(&doomed).map_err(|e| MirrorError::io(&doomed, e))?;
        // If `latest` pointed here, point it at the newest remaining snapshot.
        match self.latest()? {
            Some(l) => self.set_latest(&l.id)?,
            None => {
                let _ = fs::remove_file(self.root.join("latest"));
            }
        }
        Ok(())
    }

    fn cleanup_deleting(&self) {
        let Ok(rd) = fs::read_dir(self.snapshots_dir()) else {
            return;
        };
        for e in rd.flatten() {
            if e.file_name().as_encoded_bytes().starts_with(b".deleting-")
                && let Err(err) = flux_fs::fsops::remove_force(&e.path())
            {
                tracing::warn!("cannot finish deleting {}: {err}", e.path().display());
            }
        }
    }

    pub fn latest(&self) -> Result<Option<SnapshotMeta>, MirrorError> {
        Ok(self.snapshots()?.pop())
    }

    /// Find a snapshot by `latest`, exact id, unique id prefix, or name.
    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, MirrorError> {
        let mut all = self.snapshots()?;
        if spec == "latest" {
            return all
                .pop()
                .ok_or_else(|| MirrorError::SnapshotNotFound(spec.into()));
        }
        if let Some(i) = all.iter().position(|s| s.id == spec) {
            return Ok(all.swap_remove(i));
        }
        let matches: Vec<_> = all
            .into_iter()
            .filter(|s| s.id.starts_with(spec) || s.name.as_deref() == Some(spec))
            .collect();
        match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => Err(MirrorError::SnapshotNotFound(spec.into())),
            n => Err(MirrorError::Ambiguous(spec.into(), n)),
        }
    }

    /// A fresh snapshot id for `now` in local time, unique within this repository.
    pub fn new_snapshot_id(&self, now: Timestamp) -> String {
        let zoned = now.to_zoned(jiff::tz::TimeZone::system());
        let base = zoned.strftime("%Y-%m-%d_%H-%M-%S").to_string();
        let mut id = base.clone();
        let mut n = 2;
        while self.snapshot_dir(&id).exists() {
            id = format!("{base}_{n}");
            n += 1;
        }
        id
    }

    /// Point `latest` at `id` atomically.
    pub fn set_latest(&self, id: &str) -> Result<(), MirrorError> {
        let link = self.root.join("latest");
        let tmp = self.root.join(".latest.tmp");
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(Path::new("snapshots").join(id), &tmp)
            .map_err(|e| MirrorError::io(&tmp, e))?;
        fs::rename(&tmp, &link).map_err(|e| MirrorError::io(&link, e))
    }
}

pub fn snapshot_data(snapshot_dir: &Path) -> PathBuf {
    snapshot_dir.join(DATA)
}

pub fn snapshot_manifest(snapshot_dir: &Path) -> PathBuf {
    snapshot_dir.join(MANIFEST)
}

pub fn read_meta(snapshot_dir: &Path) -> Result<SnapshotMeta, MirrorError> {
    let p = snapshot_dir.join(META);
    let text = fs::read_to_string(&p).map_err(|e| MirrorError::io(&p, e))?;
    serde_json::from_str(&text).map_err(|e| MirrorError::Corrupt(format!("{}: {e}", p.display())))
}

pub fn write_meta(snapshot_dir: &Path, meta: &SnapshotMeta) -> Result<(), MirrorError> {
    let p = snapshot_dir.join(META);
    let text = serde_json::to_vec_pretty(meta).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
    Ok(flux_core::store::write_atomic(&p, &text, 0o644)?)
}
