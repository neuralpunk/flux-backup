//! Flux Mirror engine: every snapshot is a plain directory tree, and files unchanged since
//! the previous snapshot are hard links to it, so snapshot N+1 costs only what changed.
//!
//! Pipeline of a backup:
//! 1. **Scan** sources in parallel → sorted entries with full metadata.
//! 2. **Plan**: merge-join against the previous snapshot's manifest (streamed) → per-entry
//!    action. Bytes to copy are known before copying starts, so ETAs are real.
//! 3. **Execute** into `snapshots/.partial/`: directories, then files in parallel
//!    (hard link / reflink / `copy_file_range`), then directory metadata bottom-up.
//! 4. **Finalize**: write manifest and meta, `syncfs`, then rename `.partial` → `<id>`. The
//!    rename is the commit point; an interrupted run is resumed from `.partial`.

#![deny(clippy::unwrap_used)]

pub mod check;
pub mod exec;
pub mod manifest;
pub mod plan;
pub mod prune;
pub mod remote;
pub mod replicate;
pub mod repo;

pub use flux_fs::{fsops, scan};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

use jiff::Timestamp;

use flux_core::profile::{Profile, Target};
use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_core::{ProfileId, sys, target_check};

use crate::exec::{ExecCtx, execute};
use crate::manifest::{ManifestError, ManifestReader, ManifestWriter};
use crate::plan::plan;
use crate::repo::{
    MirrorRepo, SnapshotMeta, SnapshotStats, SnapshotStatus, snapshot_data, snapshot_manifest,
    write_meta,
};
use flux_fs::fsops::{ApplyPolicy, Copier, XattrPolicy};

pub use repo::MirrorRepo as Repo;

/// At most this many per-file errors are stored in `meta.json`.
const MAX_STORED_ERRORS: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Store(#[from] flux_core::StoreError),
    #[error(transparent)]
    Target(#[from] target_check::TargetError),
    #[error(transparent)]
    Filter(#[from] flux_core::filter::FilterError),
    #[error("no Flux backups found at {0}")]
    NoRepository(PathBuf),
    #[error("another Flux job is using {0}")]
    Locked(PathBuf),
    #[error("repository is damaged: {0}")]
    Corrupt(String),
    #[error("snapshot {0:?} not found")]
    SnapshotNotFound(String),
    #[error("{0:?} matches {1} snapshots; be more specific")]
    Ambiguous(String, usize),
    #[error("{0} is not in snapshot {1}")]
    NotInSnapshot(PathBuf, String),
    #[error(transparent)]
    Restore(flux_fs::restore::RestoreError),
    #[error("none of the source paths exist")]
    NoSources,
    #[error("not enough free space on the target: {needed} needed, {available} available")]
    NoSpace { needed: String, available: String },
    #[error("remote: {0}")]
    Remote(String),
    #[error("this engine does not support {0} yet")]
    Unsupported(&'static str),
    #[error("cancelled; the partial snapshot was kept and the next run resumes it")]
    Cancelled,
    #[error("stopped at the first error (continue_on_errors is off): {0}")]
    Aborted(String),
}

impl MirrorError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        MirrorError::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// Where a profile's Mirror repository lives, and the target root it is under.
pub fn repo_root(id: &ProfileId, profile: &Profile) -> Result<(PathBuf, PathBuf), MirrorError> {
    match &profile.target {
        Target::Local { path, .. } => {
            let k = profile.kept_as(id);
            Ok((
                MirrorRepo::locate(path, &k.host, &k.user, &k.profile),
                path.clone(),
            ))
        }
        Target::Ssh { .. } => Err(MirrorError::Unsupported("SSH targets (coming in Phase 5)")),
    }
}

/// Open the existing repository of a profile (for listing, browsing, restoring).
pub fn open_repo(id: &ProfileId, profile: &Profile) -> Result<MirrorRepo, MirrorError> {
    let (root, _) = repo_root(id, profile)?;
    MirrorRepo::open(&root)
}

pub struct BackupRequest<'a> {
    pub id: &'a ProfileId,
    pub profile: &'a Profile,
    /// Name for the new snapshot (kept forever by default).
    pub name: Option<String>,
    /// Scan and plan only.
    pub dry_run: bool,
    /// Apply the retention policy after the run, and remove old snapshots beforehand if
    /// the target lacks space.
    pub prune: bool,
    /// Sources whose drive isn't connected (left out of `profile`): their files are kept
    /// from the previous snapshot instead of looking deleted.
    pub keep: Vec<PathBuf>,
}

pub use flux_fs::outcome::BackupOutcome;

pub fn backup(
    req: &BackupRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, MirrorError> {
    backup_scanned(req, None, progress, report)
}

/// The scan of a profile's sources that [`backup`] would make (see
/// [`backup_scanned`]).
pub fn scan_for_backup(
    id: &ProfileId,
    profile: &Profile,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<scan::ScanResult, MirrorError> {
    let (root, target_path) = repo_root(id, profile)?;
    let pool = thread_pool(profile)?;
    progress.set_phase(Phase::Scanning);
    report(Event::Phase {
        phase: Phase::Scanning,
    });
    pool.install(|| scan_sources(profile, &target_path, &root, progress, report))
}

/// [`backup`], with the scan of the sources already made (by [`scan_for_backup`], just
/// before): Flux Sentry looks at that scan before a backup is written, and the folders
/// aren't walked (nor, with checksums, every file read) a second time for the backup.
pub fn backup_scanned(
    req: &BackupRequest<'_>,
    scanned: Option<scan::ScanResult>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, MirrorError> {
    let started = Timestamp::now();
    let profile = req.profile;
    progress.set_phase(Phase::Preparing);
    report(Event::Phase {
        phase: Phase::Preparing,
    });

    let (root, target_path) = repo_root(req.id, profile)?;
    if let Target::Local { path, fs_uuid } = &profile.target {
        target_check::check_local_target(path, fs_uuid.as_deref())?;
    }
    let host = sys::hostname();
    let user = sys::username();
    let repo = if req.dry_run {
        MirrorRepo::open(&root).ok()
    } else {
        Some(MirrorRepo::open_or_init(
            &root,
            req.id.as_str(),
            &host,
            &user,
        )?)
    };
    let _lock = repo.as_ref().map(MirrorRepo::lock).transpose()?;

    let opts = &profile.options;
    let xattrs = xattr_policy(profile);
    let pool = thread_pool(profile)?;

    // 1. Scan (unless it was made already).
    let scanned = match scanned {
        Some(s) => s,
        None => {
            progress.set_phase(Phase::Scanning);
            report(Event::Phase {
                phase: Phase::Scanning,
            });
            pool.install(|| scan_sources(profile, &target_path, &root, progress, report))?
        }
    };

    // 2. Plan against the previous snapshot.
    progress.set_phase(Phase::Planning);
    report(Event::Phase {
        phase: Phase::Planning,
    });
    let previous = match &repo {
        Some(r) => r.latest()?,
        None => None,
    };
    let prev_manifest = match (&repo, &previous) {
        (Some(r), Some(p)) => Some(ManifestReader::open(&r.manifest_path(&p.id))?),
        _ => None,
    };
    let mut entries = scanned.entries;
    if !req.keep.is_empty()
        && let (Some(r), Some(prev)) = (&repo, &previous)
    {
        let before = ManifestReader::read_all(&r.manifest_path(&prev.id))?;
        let kept = flux_fs::scan::carry_over(&mut entries, before, &req.keep);
        report(Event::Info {
            message: format!(
                "kept {kept} items of {} from snapshot {}: the drive isn't connected",
                list_paths(&req.keep),
                prev.id
            ),
        });
    }
    let the_plan = plan(entries, prev_manifest)?;

    if req.dry_run {
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::DryRun {
            stats: the_plan.stats,
            errors: scanned.errors,
        });
    }
    let repo = repo.expect("repository opened for real runs");

    if let Some(prev) = &previous
        && !the_plan.stats.has_changes()
        && !opts.snapshot_if_unchanged
        && req.name.is_none()
    {
        let _ = fsops::remove_force(&repo.partial_dir());
        let pruned = if req.prune {
            auto_prune(&repo, profile, report)?
        } else {
            Vec::new()
        };
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::Unchanged {
            previous: prev.id.clone(),
            pruned,
        });
    }

    let mut pruned = Vec::new();
    if let Err(e) = check_free_space(&target_path, the_plan.stats.copy_bytes) {
        if !req.prune {
            return Err(e);
        }
        pruned = prune::make_room(&repo, &profile.retention, the_plan.stats.copy_bytes, report)?;
        check_free_space(&target_path, the_plan.stats.copy_bytes)?;
    }

    // 3. Execute into .partial (resuming if an earlier run was interrupted).
    let partial = repo.partial_dir();
    let resume = partial.exists();
    if resume {
        report(Event::Info {
            message: "resuming an interrupted backup".into(),
        });
    }
    let data = snapshot_data(&partial);
    fs::create_dir_all(&data).map_err(|e| MirrorError::io(&data, e))?;

    progress.set_phase(Phase::Transferring);
    report(Event::Phase {
        phase: Phase::Transferring,
    });
    let prev_data = previous.as_ref().map(|p| repo.data_dir(&p.id));
    let policy = ApplyPolicy::new(xattrs);
    let copier = Copier::default();
    let gate = flux_fs::disk::IoGate::for_paths(
        std::iter::once(target_path.as_path())
            .chain(profile.sources.paths.iter().map(PathBuf::as_path)),
    );
    if gate.is_limited() {
        report(Event::Info {
            message: format!(
                "a spinning disk is involved: copying large files {} at a time",
                flux_fs::disk::ROTATIONAL_STREAMS
            ),
        });
    }
    let opener = flux_fs::fsops::SourceOpener::new(
        &flux_fs::scan::normalize_sources(&profile.sources.paths),
        opts.symlinks != flux_core::profile::SymlinkMode::Preserve,
    );
    let result = pool.install(|| {
        execute(&ExecCtx {
            plan: &the_plan,
            dest: &data,
            prev_data: prev_data.as_deref(),
            resume,
            policy: &policy,
            copier: &copier,
            gate: &gate,
            progress,
            report,
            continue_on_errors: opts.continue_on_errors,
            opener: &opener,
        })
    })?;

    // 4. Finalize.
    progress.set_phase(Phase::Finalizing);
    report(Event::Phase {
        phase: Phase::Finalizing,
    });
    let mut writer = ManifestWriter::create(&snapshot_manifest(&partial))?;
    let mut stats = SnapshotStats {
        new: the_plan.stats.new,
        changed: the_plan.stats.changed,
        unchanged: the_plan.stats.unchanged,
        deleted: the_plan.stats.deleted,
        copied_bytes: result.copied_bytes,
        copied_files: result.copied_files,
        linked_files: result.linked_files,
        ..Default::default()
    };
    for (e, failed) in the_plan.entries.iter().zip(&result.failed) {
        if *failed {
            continue;
        }
        writer.write(e)?;
        stats.entries += 1;
        match e.kind {
            manifest::Kind::Dir => stats.dirs += 1,
            manifest::Kind::File => {
                stats.files += 1;
                stats.total_bytes += e.size;
            }
            manifest::Kind::Symlink => stats.symlinks += 1,
            _ => stats.specials += 1,
        }
    }
    writer.finish()?;

    let mut errors = scanned.errors;
    errors.extend(result.errors);
    let errors_total = errors.len() as u64;
    errors.truncate(MAX_STORED_ERRORS);
    let id = repo.new_snapshot_id(started);
    let meta = SnapshotMeta {
        format: repo::REPO_FORMAT,
        id: id.clone(),
        profile: req.id.to_string(),
        host,
        user,
        started,
        finished: Timestamp::now(),
        status: if errors_total == 0 {
            SnapshotStatus::Complete
        } else {
            SnapshotStatus::CompleteWithErrors
        },
        name: req.name.clone(),
        pinned: false,
        sources: with_kept(&profile.sources.paths, &req.keep),
        stats,
        errors,
        errors_total,
        flux_version: flux_core::VERSION.to_owned(),
        sources_missing: req
            .keep
            .iter()
            .map(|p| flux_fs::snapshot::SourceMissing {
                path: p.clone(),
                kept_from: previous.as_ref().map(|s| s.id.clone()),
            })
            .collect(),
        note: None,
        tags: Vec::new(),
    };
    write_meta(&partial, &meta)?;

    // Make sure everything is on disk before the snapshot becomes visible.
    let dir = fs::File::open(&partial).map_err(|e| MirrorError::io(&partial, e))?;
    rustix::fs::syncfs(&dir).map_err(|e| MirrorError::io(&partial, e.into()))?;
    let final_dir = repo.snapshot_dir(&id);
    fs::rename(&partial, &final_dir).map_err(|e| MirrorError::io(&final_dir, e))?;
    repo.set_latest(&id)?;
    if let Ok(d) = fs::File::open(repo.snapshots_dir()) {
        let _ = d.sync_all();
    }

    if req.prune {
        pruned.extend(auto_prune(&repo, profile, report)?);
    }

    progress.set_phase(Phase::Done);
    report(Event::Phase { phase: Phase::Done });
    debug_assert!(progress.entries_done.load(Relaxed) <= progress.entries_total.load(Relaxed));
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned,
    })
}

fn auto_prune(
    repo: &MirrorRepo,
    profile: &Profile,
    report: Reporter<'_>,
) -> Result<Vec<String>, MirrorError> {
    let r = prune::prune_locked(repo, &profile.retention, false, report)?;
    if r.space_floor_unmet {
        report(Event::Warning {
            path: repo.root().display().to_string(),
            message:
                "free space is below the profile's minimum and no more snapshots may be removed"
                    .into(),
        });
    }
    Ok(r.removed.into_iter().chain(r.removed_for_space).collect())
}

fn xattr_policy(profile: &Profile) -> XattrPolicy {
    flux_fs::scan::xattr_policy(profile)
}

fn thread_pool(profile: &Profile) -> Result<rayon::ThreadPool, MirrorError> {
    flux_fs::scan::thread_pool(profile).map_err(scan_err)
}

fn scan_err(e: flux_fs::scan::ProfileScanError) -> MirrorError {
    use flux_fs::scan::ProfileScanError as E;
    match e {
        E::Filter(f) => MirrorError::Filter(f),
        E::NoSources => MirrorError::NoSources,
        E::Cancelled => MirrorError::Cancelled,
        E::Pool(p) => MirrorError::Corrupt(format!("thread pool: {p}")),
    }
}

/// Scan a profile's sources exactly as a backup would, never descending into the backup
/// repository itself.
fn scan_sources(
    profile: &Profile,
    target_path: &Path,
    repo_root: &Path,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<scan::ScanResult, MirrorError> {
    let never = [
        target_path.join("flux-backup"),
        target_path.join("flux-vault"),
        repo_root.to_owned(),
    ];
    flux_fs::scan::scan_profile(profile, &never, progress, report).map_err(scan_err)
}

/// Scan a profile's sources now, for comparing the live system with a snapshot.
pub fn scan_live(
    id: &ProfileId,
    profile: &Profile,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<scan::ScanResult, MirrorError> {
    let (root, target_path) = repo_root(id, profile)?;
    let pool = thread_pool(profile)?;
    pool.install(|| scan_sources(profile, &target_path, &root, progress, report))
}

/// The entries of a Mirror snapshot, sorted by path (for diff, find and history).
pub fn entries_of<'a>(
    repo: &'a MirrorRepo,
    snap: &SnapshotMeta,
) -> Result<Box<dyn Iterator<Item = Result<manifest::Entry, MirrorError>> + Send + 'a>, MirrorError>
{
    let r = ManifestReader::open(&repo.manifest_path(&snap.id))?;
    Ok(Box::new(r.map(|e| e.map_err(MirrorError::from))))
}

/// A Mirror snapshot as a restore source: entries from its manifest, contents copied from
/// its data tree.
pub struct MirrorSource<'a> {
    repo: &'a MirrorRepo,
    id: String,
}

impl<'a> MirrorSource<'a> {
    pub fn new(repo: &'a MirrorRepo, snapshot_id: &str) -> Self {
        Self {
            repo,
            id: snapshot_id.to_owned(),
        }
    }
}

impl flux_fs::restore::RestoreSource for MirrorSource<'_> {
    fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<manifest::Entry>> + '_>> {
        let r =
            ManifestReader::open(&self.repo.manifest_path(&self.id)).map_err(io::Error::other)?;
        Ok(Box::new(r.map(|e| e.map_err(io::Error::other))))
    }

    fn write_file(
        &self,
        e: &manifest::Entry,
        out: &std::fs::File,
        copier: &Copier,
        progress: &Progress,
    ) -> io::Result<u64> {
        copier.copy_below(&self.repo.data_dir(&self.id), e.rel_path(), out, progress)
    }
}

/// Restore from a Mirror snapshot.
pub fn restore(
    repo: &MirrorRepo,
    req: &flux_fs::restore::RestoreRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<flux_fs::restore::RestoreOutcome, MirrorError> {
    use flux_fs::restore::RestoreError;
    flux_fs::restore::restore(
        &MirrorSource::new(repo, &req.snapshot.id),
        req,
        progress,
        report,
    )
    .map_err(|e| match e {
        RestoreError::NotInSnapshot(p, s) => MirrorError::NotInSnapshot(p, s),
        RestoreError::Cancelled => MirrorError::Cancelled,
        other => MirrorError::Restore(other),
    })
}

fn check_free_space(target: &Path, needed: u64) -> Result<(), MirrorError> {
    let Ok(vfs) = rustix::fs::statvfs(target) else {
        return Ok(());
    };
    let available = vfs.f_bavail.saturating_mul(vfs.f_frsize);
    // Directories and metadata take space too; keep a small margin.
    let needed_with_margin = needed + needed / 50 + (16 << 20);
    if available < needed_with_margin {
        return Err(MirrorError::NoSpace {
            needed: flux_core::ByteSize(needed).display_approx(),
            available: flux_core::ByteSize(available).display_approx(),
        });
    }
    Ok(())
}

/// The sources a snapshot covers: those scanned and those kept from before.
pub fn with_kept(sources: &[PathBuf], kept: &[PathBuf]) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = sources.iter().chain(kept).cloned().collect();
    all.sort();
    all.dedup();
    all
}

/// "/mnt/photos and /mnt/music" (for messages).
pub fn list_paths(paths: &[PathBuf]) -> String {
    let v: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    match v.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
