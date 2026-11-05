//! One handle for a profile's backups, whatever the engine and target. The CLI and the GUI
//! use this for everything except taking backups (see [`crate::run_backup`]).

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use flux_core::profile::{Engine, Profile, Retention, Target};
use flux_core::progress::{Progress, Reporter};
use flux_core::retention::Decision;
use flux_core::{Paths, ProfileId, sys, target_check};
use flux_fs::entry::Entry;
use flux_fs::outcome::CheckSummary;
use flux_fs::restore::{RestoreError, RestoreOutcome, RestoreRequest};
use flux_fs::snapshot::SnapshotMeta;
use flux_mirror::MirrorError;
use flux_mirror::remote::{RemoteMirror, RemoteSource};
use flux_mirror::repo::MirrorRepo;
use flux_transport::{SftpPool, SshTarget};
use flux_vault::{Owner, VaultError, VaultRepo};

use crate::password::{self, Ask, PasswordError, Purpose};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error(transparent)]
    Mirror(#[from] MirrorError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Target(#[from] target_check::TargetError),
    #[error(transparent)]
    Restore(RestoreError),
    #[error("{0} is not in snapshot {1}")]
    NotInSnapshot(PathBuf, String),
    #[error("profile {0} has no backups yet")]
    Empty(String),
}

impl RepoError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            RepoError::Mirror(MirrorError::Cancelled)
                | RepoError::Vault(VaultError::Cancelled)
                | RepoError::Restore(RestoreError::Cancelled)
        )
    }
}

impl From<flux_fs::scan::ProfileScanError> for RepoError {
    fn from(e: flux_fs::scan::ProfileScanError) -> Self {
        RepoError::Vault(e.into())
    }
}

pub enum AnyRepo {
    Mirror(MirrorRepo),
    /// Mirror on an SSH target.
    MirrorRemote(Box<RemoteMirror>),
    Vault {
        repo: Box<VaultRepo>,
        owner: Owner,
    },
}

/// SFTP sessions kept per remote repository.
const SFTP_SESSIONS: usize = 4;

/// Connection to a profile's SSH target (`None` for local targets). `batch`: nobody can
/// answer ssh prompts (scheduled runs).
pub fn sftp_pool(paths: &Paths, profile: &Profile, batch: bool) -> Option<Arc<SftpPool>> {
    let t = SshTarget::from_target(&profile.target, &paths.runtime_dir, batch)?;
    Some(Arc::new(SftpPool::new(t, SFTP_SESSIONS)))
}

/// Owner of a profile's snapshots in a (shared) Vault.
pub fn owner_for(id: &ProfileId) -> Owner {
    Owner {
        profile: id.to_string(),
        host: sys::hostname(),
        user: sys::username(),
    }
}

/// Root of a profile's Vault: a local path, or a path on the SSH server.
pub fn vault_root(profile: &Profile) -> PathBuf {
    match &profile.target {
        Target::Local { path, .. } | Target::Ssh { path, .. } => flux_vault::local_root(path),
    }
}

/// Storage for a profile's Vault: local directory, or SFTP through `pool`.
pub fn vault_backend(
    profile: &Profile,
    pool: Option<Arc<SftpPool>>,
) -> Box<dyn flux_vault::Backend> {
    let root = vault_root(profile);
    match (pool, &profile.target) {
        (
            Some(p),
            Target::Ssh {
                bandwidth_limit, ..
            },
        ) => Box::new(flux_vault::backend::SftpBackend::new(
            p,
            root.display().to_string(),
            bandwidth_limit.map(|b| b.bytes()),
        )),
        _ => Box::new(flux_vault::LocalBackend::new(&root)),
    }
}

/// Open (or, for backups, create) a profile's Vault.
pub fn open_vault(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    create: bool,
    ask: Option<Ask<'_>>,
) -> Result<VaultRepo, RepoError> {
    if let Target::Local { path, fs_uuid } = &profile.target {
        target_check::check_local_target(path, fs_uuid.as_deref())?;
    }
    let pool = sftp_pool(paths, profile, ask.is_none());
    let backend = || vault_backend(profile, pool.clone());
    let encrypted = profile.vault.as_ref().is_none_or(|v| v.encryption);
    if !VaultRepo::exists(backend().as_ref())? {
        if !create {
            return Err(RepoError::Empty(id.to_string()));
        }
        let pw = if encrypted {
            Some(password::obtain(id, profile, Purpose::Create, ask)?)
        } else {
            None
        };
        return Ok(VaultRepo::init(
            backend(),
            pw.as_deref().map(|s| s.as_str()),
            flux_vault::crypto::KdfParams::DEFAULT,
        )?);
    }
    // Only ask for a password if the repository is actually encrypted.
    match VaultRepo::open(backend(), None) {
        Err(VaultError::PasswordRequired) => {
            let pw = password::obtain(id, profile, Purpose::Open, ask)?;
            Ok(VaultRepo::open(backend(), Some(&pw))?)
        }
        r => Ok(r?),
    }
}

/// What prune did, in engine-neutral terms.
#[derive(Debug, Serialize)]
pub struct PruneSummary {
    pub decisions: Vec<Decision>,
    pub removed: Vec<String>,
    pub removed_for_space: Vec<String>,
    pub space_floor_unmet: bool,
    /// Bytes reclaimed by Vault garbage collection.
    pub bytes_freed: Option<u64>,
    pub dry_run: bool,
}

type Entries<'a> = Box<dyn Iterator<Item = Result<Entry, RepoError>> + Send + 'a>;

impl AnyRepo {
    /// Open the existing backups of a profile. `ask: None` means nobody can be prompted
    /// (no password prompts, ssh in batch mode).
    pub fn open(
        paths: &Paths,
        id: &ProfileId,
        profile: &Profile,
        ask: Option<Ask<'_>>,
    ) -> Result<Self, RepoError> {
        match (profile.engine, &profile.target) {
            (Engine::Mirror, Target::Ssh { path, .. }) => {
                let pool = sftp_pool(paths, profile, ask.is_none()).expect("ssh target");
                let root =
                    RemoteMirror::locate(path, &sys::hostname(), &sys::username(), id.as_str());
                match RemoteMirror::open(pool, root, &paths.cache_dir) {
                    Ok(r) => Ok(AnyRepo::MirrorRemote(Box::new(r))),
                    Err(MirrorError::NoRepository(_)) => Err(RepoError::Empty(id.to_string())),
                    Err(e) => Err(e.into()),
                }
            }
            (Engine::Mirror, Target::Local { .. }) => match flux_mirror::open_repo(id, profile) {
                Ok(r) => Ok(AnyRepo::Mirror(r)),
                Err(MirrorError::NoRepository(_)) => Err(RepoError::Empty(id.to_string())),
                Err(e) => Err(e.into()),
            },
            (Engine::Vault, _) => Ok(AnyRepo::Vault {
                repo: Box::new(open_vault(paths, id, profile, false, ask)?),
                owner: owner_for(id),
            }),
        }
    }

    pub fn engine(&self) -> Engine {
        match self {
            AnyRepo::Mirror(_) | AnyRepo::MirrorRemote(_) => Engine::Mirror,
            AnyRepo::Vault { .. } => Engine::Vault,
        }
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, RepoError> {
        Ok(match self {
            AnyRepo::Mirror(r) => r.snapshots()?,
            AnyRepo::MirrorRemote(r) => r.snapshots()?,
            AnyRepo::Vault { repo, owner } => repo.snapshots(owner)?,
        })
    }

    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, RepoError> {
        Ok(match self {
            AnyRepo::Mirror(r) => r.find(spec)?,
            AnyRepo::MirrorRemote(r) => r.find(spec)?,
            AnyRepo::Vault { repo, owner } => repo.find(owner, spec)?,
        })
    }

    /// The snapshot's entries, sorted by path.
    pub fn entries(&self, snap: &SnapshotMeta) -> Result<Entries<'_>, RepoError> {
        match self {
            AnyRepo::Mirror(r) => {
                let it = flux_mirror::entries_of(r, snap)?;
                Ok(Box::new(it.map(|e| e.map_err(RepoError::from))))
            }
            AnyRepo::MirrorRemote(r) => {
                let it = r.entries(&snap.id)?;
                Ok(Box::new(
                    it.map(|e| e.map_err(|e| RepoError::Mirror(e.into()))),
                ))
            }
            AnyRepo::Vault { repo, .. } => {
                let v = flux_vault::entries_of(repo, &snap.id)?;
                Ok(Box::new(v.into_iter().map(Ok)))
            }
        }
    }

    pub fn restore(
        &self,
        req: &RestoreRequest<'_>,
        progress: &Progress,
        report: Reporter<'_>,
    ) -> Result<RestoreOutcome, RepoError> {
        let map = |e: RestoreError| match e {
            RestoreError::NotInSnapshot(p, s) => RepoError::NotInSnapshot(p, s),
            other => RepoError::Restore(other),
        };
        match self {
            AnyRepo::Mirror(r) => match flux_mirror::restore(r, req, progress, report) {
                Err(MirrorError::NotInSnapshot(p, s)) => Err(RepoError::NotInSnapshot(p, s)),
                other => Ok(other?),
            },
            AnyRepo::MirrorRemote(r) => {
                let source = RemoteSource::new(r, &req.snapshot.id);
                flux_fs::restore::restore(&source, req, progress, report).map_err(map)
            }
            AnyRepo::Vault { repo, .. } => {
                let _lock = repo.lock(false)?;
                let source = flux_vault::VaultSource::new(repo, &req.snapshot.id)?;
                flux_fs::restore::restore(&source, req, progress, report).map_err(map)
            }
        }
    }

    /// Set or clear a snapshot's name, or its pin.
    pub fn update(
        &self,
        id: &str,
        name: Option<Option<String>>,
        pinned: Option<bool>,
    ) -> Result<SnapshotMeta, RepoError> {
        let apply = |m: &mut SnapshotMeta| {
            if let Some(n) = &name {
                m.name = n.clone();
            }
            if let Some(p) = pinned {
                m.pinned = p;
            }
        };
        match self {
            AnyRepo::Mirror(r) => {
                let _l = r.lock()?;
                Ok(r.update_meta(id, apply)?)
            }
            AnyRepo::MirrorRemote(r) => {
                let _l = r.lock()?;
                Ok(r.update_meta(id, apply)?)
            }
            AnyRepo::Vault { repo, .. } => {
                let _l = repo.lock(false)?;
                let mut rec = repo.read_record(id)?;
                apply(&mut rec.meta);
                repo.write_record(&rec)?;
                Ok(rec.meta)
            }
        }
    }

    /// Delete snapshots and free their space now. For a Vault, data that other snapshots
    /// still use is kept; the bytes freed are returned. (Mirror snapshots free their space
    /// as their folders go, and report `None`.)
    pub fn delete(&self, ids: &[&str], report: Reporter<'_>) -> Result<Option<u64>, RepoError> {
        match self {
            AnyRepo::Mirror(r) => {
                let _l = r.lock()?;
                for id in ids {
                    r.delete_snapshot(id)?;
                }
                Ok(None)
            }
            AnyRepo::MirrorRemote(r) => {
                let _l = r.lock()?;
                for id in ids {
                    r.delete_snapshot(id)?;
                }
                Ok(None)
            }
            AnyRepo::Vault { repo, .. } => {
                Ok(Some(flux_vault::gc::delete(repo, ids, report)?.bytes_freed))
            }
        }
    }

    pub fn prune(
        &self,
        policy: &Retention,
        dry_run: bool,
        report: Reporter<'_>,
    ) -> Result<PruneSummary, RepoError> {
        match self {
            AnyRepo::Mirror(r) => {
                let _l = r.lock()?;
                let p = flux_mirror::prune::prune_locked(r, policy, dry_run, report)?;
                Ok(PruneSummary {
                    decisions: p.decisions,
                    removed: p.removed,
                    removed_for_space: p.removed_for_space,
                    space_floor_unmet: p.space_floor_unmet,
                    bytes_freed: None,
                    dry_run,
                })
            }
            AnyRepo::MirrorRemote(r) => {
                let _l = r.lock()?;
                let (decisions, removed) =
                    flux_mirror::remote::prune_locked(r, policy, dry_run, report)?;
                Ok(PruneSummary {
                    decisions,
                    removed,
                    removed_for_space: Vec::new(),
                    space_floor_unmet: false,
                    bytes_freed: None,
                    dry_run,
                })
            }
            AnyRepo::Vault { repo, owner } => {
                let p = flux_vault::gc::prune(repo, owner, policy, dry_run, report)?;
                Ok(PruneSummary {
                    decisions: p.decisions,
                    removed: p.removed,
                    removed_for_space: Vec::new(),
                    space_floor_unmet: false,
                    bytes_freed: p.gc.map(|g| g.bytes_freed),
                    dry_run,
                })
            }
        }
    }

    pub fn check(
        &self,
        read_data: bool,
        progress: &Progress,
        report: Reporter<'_>,
    ) -> Result<CheckSummary, RepoError> {
        match self {
            AnyRepo::Mirror(r) => Ok(flux_mirror::check::check(r, read_data, progress)?),
            AnyRepo::MirrorRemote(r) => Ok(flux_mirror::remote::check(r, read_data, progress)?),
            AnyRepo::Vault { repo, .. } => {
                let c = flux_vault::check::check(repo, read_data, progress, report)?;
                Ok(CheckSummary {
                    snapshots: c.snapshots,
                    checked: c.blobs,
                    bytes_read: c.bytes_read,
                    errors: c.errors,
                    warnings: c.warnings,
                })
            }
        }
    }

    /// Where a snapshot is stored, for display.
    pub fn location(&self, snap: &SnapshotMeta) -> String {
        match self {
            AnyRepo::Mirror(r) => r.snapshot_dir(&snap.id).display().to_string(),
            AnyRepo::MirrorRemote(r) => format!("{}/snapshots/{}", r.describe(), snap.id),
            AnyRepo::Vault { repo, .. } => {
                format!("{} (snapshot {})", repo.backend().describe(), snap.id)
            }
        }
    }

    pub fn vault(&self) -> Option<&VaultRepo> {
        match self {
            AnyRepo::Vault { repo, .. } => Some(repo.as_ref()),
            _ => None,
        }
    }
}

/// Scan a profile's sources as a backup would (for `diff --live`).
pub fn scan_live(
    id: &ProfileId,
    profile: &Profile,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<Vec<Entry>, RepoError> {
    match (profile.engine, &profile.target) {
        (Engine::Mirror, Target::Local { .. }) => {
            Ok(flux_mirror::scan_live(id, profile, progress, report)?.entries)
        }
        (engine, target) => {
            let never = match (engine, target) {
                (Engine::Vault, Target::Local { .. }) => {
                    let root = vault_root(profile);
                    vec![root.clone(), root.with_file_name("flux-backup")]
                }
                _ => Vec::new(),
            };
            let pool = flux_fs::scan::thread_pool(profile)?;
            Ok(pool
                .install(|| flux_fs::scan::scan_profile(profile, &never, progress, report))?
                .entries)
        }
    }
}

impl From<RepoError> for io::Error {
    fn from(e: RepoError) -> Self {
        io::Error::other(e)
    }
}
