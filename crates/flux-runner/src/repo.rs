//! One handle for a profile's backups, whatever the engine and target. The CLI and the GUI
//! use this for everything except taking backups (see [`crate::run_backup`]).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use flux_compat::CompatError;
use flux_compat::borg::{BorgRepo, BorgSource};
use flux_compat::rsync::{RsyncRepo, RsyncSource};
use flux_core::profile::{Engine, Profile, Retention, Target};
use flux_core::progress::{Progress, Reporter};
use flux_core::retention::Decision;
use flux_core::{Paths, ProfileId, target_check};
use flux_fs::entry::Entry;
use flux_fs::outcome::CheckSummary;
use flux_fs::restore::{RestoreError, RestoreOutcome, RestoreRequest};
use flux_fs::snapshot::SnapshotMeta;
use flux_mirror::MirrorError;
use flux_mirror::remote::{RemoteMirror, RemoteSource};
use flux_mirror::repo::MirrorRepo;
use flux_transport::{SftpPool, SshTarget};
use flux_vault::crypto::UnlockSecret;
use flux_vault::{Owner, VaultError, VaultRepo};

use crate::password::{self, Ask, PasswordError, Purpose};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error(transparent)]
    Mirror(#[from] MirrorError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Compat(#[from] CompatError),
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
    #[error(
        "this Vault's password works only together with its unlock file, {0}, which isn't there: connect the drive or stick it's on"
    )]
    UnlockFileMissing(PathBuf),
    #[error("can't read the unlock file {0}: {1}")]
    UnlockFile(PathBuf, std::io::Error),
    #[error(
        "this backup is set to be encrypted, but the Vault at {0} says it is not. Nothing was written. If the Vault never was encrypted, turn encryption off for this backup (`encryption = false` under [vault] in its settings). If it was, someone has changed the Vault: don't back up to it until you know why"
    )]
    NotEncrypted(String),
}

impl RepoError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            RepoError::Mirror(MirrorError::Cancelled)
                | RepoError::Vault(VaultError::Cancelled)
                | RepoError::Restore(RestoreError::Cancelled)
                | RepoError::Compat(CompatError::Cancelled)
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
    /// Compatibility mode: plain folders written by rsync (local or SSH).
    Rsync(Box<RsyncRepo>),
    /// Compatibility mode: a Borg repository (local or SSH).
    Borg(Box<BorgRepo>),
}

/// SFTP sessions kept per remote repository.
const SFTP_SESSIONS: usize = 4;

/// Connection to a profile's SSH target (`None` for local targets). `batch`: nobody can
/// answer ssh prompts (scheduled runs).
pub fn sftp_pool(paths: &Paths, profile: &Profile, batch: bool) -> Option<Arc<SftpPool>> {
    let t = SshTarget::from_target(&profile.target, &paths.runtime_dir, batch)?;
    Some(Arc::new(SftpPool::new(t, SFTP_SESSIONS)))
}

/// Owner of a profile's snapshots in a (shared) Vault: this computer and user, or the
/// ones a recovered backup was made as ([`Profile::kept_as`]).
pub fn owner_for(id: &ProfileId, profile: &Profile) -> Owner {
    let k = profile.kept_as(id);
    Owner {
        profile: k.profile,
        host: k.host,
        user: k.user,
    }
}

/// Owner of the snapshots a profile sees: a copy of the main destination
/// ([`Profile::replica`]) holds the main profile's snapshots.
pub fn owner_of(id: &ProfileId, profile: &Profile) -> Owner {
    match id.split_destination() {
        Some((base, _)) if profile.replica => owner_for(&base, profile),
        _ => owner_for(id, profile),
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
        ) => {
            let b = flux_vault::backend::SftpBackend::new(
                p,
                root.display().to_string(),
                bandwidth_limit.map(|b| b.bytes()),
            );
            if profile.network.bandwidth.is_empty() {
                Box::new(b)
            } else {
                // Limits by time of day: asked as the backup runs.
                let (network, default) = (profile.network.clone(), *bandwidth_limit);
                Box::new(b.with_rate(Box::new(move || {
                    let now = jiff::Zoned::now().datetime();
                    network.limit_at(&now, default).map(|b| b.bytes())
                })))
            }
        }
        _ => Box::new(flux_vault::LocalBackend::new(&root)),
    }
}

/// The profile with its server target's upload limit set to the one in force now (from
/// `[network] bandwidth`), for engines that take the limit when they start.
pub fn with_bandwidth_now(profile: &Profile) -> std::borrow::Cow<'_, Profile> {
    let Target::Ssh {
        bandwidth_limit, ..
    } = &profile.target
    else {
        return std::borrow::Cow::Borrowed(profile);
    };
    if profile.network.bandwidth.is_empty() {
        return std::borrow::Cow::Borrowed(profile);
    }
    let now = profile
        .network
        .limit_at(&jiff::Zoned::now().datetime(), *bandwidth_limit);
    let mut p = profile.clone();
    if let Target::Ssh {
        bandwidth_limit, ..
    } = &mut p.target
    {
        *bandwidth_limit = now;
    }
    std::borrow::Cow::Owned(p)
}

/// The largest unlock file Flux reads (any file works as one; Flux makes 64-byte ones).
const UNLOCK_FILE_MAX: u64 = 1 << 20;

/// The secret of the profile's unlock file, if it has one and the file is there.
pub fn unlock_secret(profile: &Profile) -> Result<Option<UnlockSecret>, RepoError> {
    let Some(path) = profile.vault.as_ref().and_then(|v| v.unlock_file.as_ref()) else {
        return Ok(None);
    };
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(RepoError::UnlockFile(path.clone(), e)),
        Ok(m) if !m.is_file() || m.len() > UNLOCK_FILE_MAX => {
            return Err(RepoError::UnlockFile(
                path.clone(),
                std::io::Error::other("not a file of at most 1 MiB"),
            ));
        }
        Ok(_) => {}
    }
    let bytes = std::fs::read(path).map_err(|e| RepoError::UnlockFile(path.clone(), e))?;
    Ok(Some(flux_vault::crypto::unlock_secret(&bytes)))
}

/// Create a new unlock file at `path` (64 random bytes, private, never overwriting).
pub fn create_unlock_file(path: &Path) -> Result<UnlockSecret, RepoError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes: [u8; 64] = flux_vault::crypto::random().map_err(VaultError::from)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| RepoError::UnlockFile(path.to_owned(), e))?;
    f.write_all(&bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| RepoError::UnlockFile(path.to_owned(), e))?;
    Ok(flux_vault::crypto::unlock_secret(&bytes))
}

/// Apply the profile's Vault settings that matter while writing: the password hashing for
/// new passwords, and padding.
fn configure(repo: &mut VaultRepo, profile: &Profile) {
    if let Some(v) = &profile.vault {
        repo.kdf = flux_vault::crypto::KdfParams::of(v.hashing);
        repo.pad_packs = v.hide_sizes && v.encryption;
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
        let unlock = if encrypted {
            unlock_secret(profile)?
        } else {
            None
        };
        let mut repo = VaultRepo::init_with(
            backend(),
            pw.as_deref().map(|s| s.as_str()),
            profile
                .vault
                .as_ref()
                .map_or(flux_vault::crypto::KdfParams::DEFAULT, |v| {
                    flux_vault::crypto::KdfParams::of(v.hashing)
                }),
            unlock,
        )?;
        configure(&mut repo, profile);
        return Ok(repo);
    }
    // Only ask for a password if the repository is actually encrypted.
    let mut repo = match VaultRepo::open(backend(), None) {
        Err(VaultError::PasswordRequired) => {
            let pw = password::obtain(id, profile, Purpose::Open, ask)?;
            open_vault_with(backend(), profile, &pw)?
        }
        r => r?,
    };
    // Whether a Vault is encrypted is read from the Vault itself, which whoever holds the
    // drive or server can rewrite. A backup that is meant to be encrypted never goes on
    // in the clear.
    if encrypted && !repo.config.encrypted {
        return Err(RepoError::NotEncrypted(backend().describe()));
    }
    configure(&mut repo, profile);
    Ok(repo)
}

/// Open an existing encrypted Vault with `password` and the profile's unlock file (if it
/// has one). A missing unlock file is [`RepoError::UnlockFileMissing`], naming it.
pub fn open_vault_with(
    backend: Box<dyn flux_vault::Backend>,
    profile: &Profile,
    password: &str,
) -> Result<VaultRepo, RepoError> {
    let unlock = unlock_secret(profile)?;
    match VaultRepo::open_with(backend, Some(password), unlock) {
        Err(VaultError::UnlockFileNeeded) => Err(
            match profile.vault.as_ref().and_then(|v| v.unlock_file.clone()) {
                Some(f) => RepoError::UnlockFileMissing(f),
                None => VaultError::UnlockFileNeeded.into(),
            },
        ),
        r => {
            let mut repo = r?;
            configure(&mut repo, profile);
            Ok(repo)
        }
    }
}

/// A profile's Borg repository, with its passphrase when it's encrypted. `create`: a
/// backup is about to create the repository if there is none (asks for a new
/// passphrase); otherwise a missing repository is [`RepoError::Empty`].
pub fn open_borg(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    create: bool,
    ask: Option<Ask<'_>>,
) -> Result<BorgRepo, RepoError> {
    let batch = ask.is_none();
    let bare = BorgRepo::for_profile(paths, id, profile, None, batch)?;
    let exists = bare.exists();
    if !exists && !create {
        return Err(RepoError::Empty(id.to_string()));
    }
    let passphrase = match profile.password_source() {
        Some(_) => Some(password::obtain(
            id,
            profile,
            if exists {
                Purpose::Open
            } else {
                Purpose::Create
            },
            ask,
        )?),
        None => None,
    };
    Ok(BorgRepo::for_profile(
        paths, id, profile, passphrase, batch,
    )?)
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

pub type Entries<'a> = Box<dyn Iterator<Item = Result<Entry, RepoError>> + Send + 'a>;

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
                let k = profile.kept_as(id);
                let root = RemoteMirror::locate(path, &k.host, &k.user, &k.profile);
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
                owner: owner_of(id, profile),
            }),
            (Engine::Rsync, _) => {
                match RsyncRepo::open_profile(paths, id, profile, ask.is_none()) {
                    Ok(r) => Ok(AnyRepo::Rsync(Box::new(r))),
                    Err(CompatError::NoRepository(_)) => Err(RepoError::Empty(id.to_string())),
                    Err(e) => Err(e.into()),
                }
            }
            (Engine::Borg, _) => Ok(AnyRepo::Borg(Box::new(open_borg(
                paths, id, profile, false, ask,
            )?))),
        }
    }

    pub fn engine(&self) -> Engine {
        match self {
            AnyRepo::Mirror(_) | AnyRepo::MirrorRemote(_) => Engine::Mirror,
            AnyRepo::Vault { .. } => Engine::Vault,
            AnyRepo::Rsync(_) => Engine::Rsync,
            AnyRepo::Borg(_) => Engine::Borg,
        }
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, RepoError> {
        Ok(match self {
            AnyRepo::Mirror(r) => r.snapshots()?,
            AnyRepo::MirrorRemote(r) => r.snapshots()?,
            AnyRepo::Vault { repo, owner } => repo.snapshots(owner)?,
            AnyRepo::Rsync(r) => r.snapshots()?,
            AnyRepo::Borg(r) => r.snapshots()?,
        })
    }

    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, RepoError> {
        Ok(match self {
            AnyRepo::Mirror(r) => r.find(spec)?,
            AnyRepo::MirrorRemote(r) => r.find(spec)?,
            AnyRepo::Vault { repo, owner } => repo.find(owner, spec)?,
            AnyRepo::Rsync(r) => r.find(spec)?,
            AnyRepo::Borg(r) => r.find(spec)?,
        })
    }

    /// Whether `id` is the full id of a Vault snapshot whose record is damaged: it isn't
    /// listed and can't be restored, and pruning refuses to delete anything while it's
    /// there. It can be deleted by that id.
    pub fn is_damaged_snapshot(&self, id: &str) -> bool {
        match self {
            AnyRepo::Vault { repo, .. } => repo.is_damaged_record(id),
            _ => false,
        }
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
            AnyRepo::Rsync(r) => Ok(Box::new(r.entries(snap)?.into_iter().map(Ok))),
            AnyRepo::Borg(r) => Ok(Box::new(r.entries(snap)?.into_iter().map(Ok))),
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
            AnyRepo::Rsync(r) => {
                let source = RsyncSource::new(r, req.snapshot);
                flux_fs::restore::restore(&source, req, progress, report).map_err(map)
            }
            AnyRepo::Borg(r) => {
                let staging = req.to.clone().unwrap_or_else(|| r.staging_dir().to_owned());
                let source = BorgSource::prepare(
                    r,
                    req.snapshot,
                    &req.paths,
                    &staging,
                    req.dry_run,
                    progress,
                    report,
                )?;
                let out = flux_fs::restore::restore(&source, req, progress, report)
                    .map_err(map)
                    .map(|mut out| {
                        // What borg couldn't extract whole wasn't restored: say so.
                        out.errors.extend(source.errors.iter().cloned());
                        out
                    });
                drop(source);
                out
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
        self.change(
            id,
            &flux_fs::snapshot::SnapshotChange {
                name,
                pinned,
                ..Default::default()
            },
        )
    }

    /// Change a snapshot's name, pin, note or tags (stored with the snapshot, whatever
    /// the engine: Borg keeps them in the archive's comment).
    pub fn change(
        &self,
        id: &str,
        change: &flux_fs::snapshot::SnapshotChange,
    ) -> Result<SnapshotMeta, RepoError> {
        let apply = |m: &mut SnapshotMeta| change.apply(m);
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
            AnyRepo::Rsync(r) => {
                let _l = r.lock()?;
                Ok(r.update_meta(id, apply)?)
            }
            AnyRepo::Borg(r) => Ok(r.update_meta(id, apply)?),
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
            AnyRepo::Rsync(r) => {
                let _l = r.lock()?;
                for id in ids {
                    r.delete_snapshot(id)?;
                }
                Ok(None)
            }
            AnyRepo::Borg(r) => Ok(r.delete(ids, report)?),
        }
    }

    /// Give back the space of data no snapshot uses any more (what forgotten snapshots
    /// leave behind in a Vault), without forgetting any snapshot. The bytes freed; `None`
    /// for formats that free space as snapshots go.
    pub fn free_unused(&self, report: Reporter<'_>) -> Result<Option<u64>, RepoError> {
        match self {
            AnyRepo::Vault { repo, .. } => {
                Ok(Some(flux_vault::gc::collect(repo, report)?.bytes_freed))
            }
            _ => Ok(None),
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
            AnyRepo::Borg(r) => {
                let p = flux_compat::borg::prune_repo(r, policy, dry_run, report)?;
                Ok(PruneSummary {
                    decisions: p.decisions,
                    removed: p.removed,
                    removed_for_space: p.removed_for_space,
                    space_floor_unmet: p.space_floor_unmet,
                    bytes_freed: None,
                    dry_run,
                })
            }
            AnyRepo::Rsync(r) => {
                let _l = r.lock()?;
                let p = flux_compat::rsync::prune_locked(r, policy, dry_run, report)?;
                Ok(PruneSummary {
                    decisions: p.decisions,
                    removed: p.removed,
                    removed_for_space: p.removed_for_space,
                    space_floor_unmet: p.space_floor_unmet,
                    bytes_freed: None,
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
            AnyRepo::Rsync(r) => Ok(flux_compat::rsync::check(r, read_data, progress)?),
            AnyRepo::Borg(r) => Ok(flux_compat::borg::check(r, read_data, progress)?),
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
            AnyRepo::Rsync(r) => r.snapshot_location(&snap.id),
            AnyRepo::Borg(r) => format!("{}::{}", r.describe(), snap.id),
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
                (Engine::Rsync, _) => flux_compat::rsync::never_paths(profile),
                (Engine::Borg, _) => flux_compat::borg::never_paths(profile),
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

#[cfg(test)]
mod bandwidth_tests {
    use super::*;
    use flux_core::network::BandwidthWindow;

    #[test]
    fn engines_get_the_limit_in_force() {
        let mut p = Profile::new(
            "t",
            Target::Ssh {
                host: "nas".into(),
                user: None,
                port: None,
                path: "/srv".into(),
                identity_file: None,
                ssh_options: Vec::new(),
                bandwidth_limit: Some("10MB".parse().unwrap()),
            },
            vec!["/home/me".into()],
        );
        assert!(matches!(
            with_bandwidth_now(&p),
            std::borrow::Cow::Borrowed(_)
        ));
        // Two windows that cover the whole day, at 1 MB/s: whatever the time now.
        let w = |from: &str, to: &str| BandwidthWindow {
            days: "*".into(),
            from: from.into(),
            to: to.into(),
            limit: Some("1MB".parse().unwrap()),
        };
        p.network.bandwidth = vec![w("00:00", "12:00"), w("12:00", "00:00")];
        let now = with_bandwidth_now(&p);
        let Target::Ssh {
            bandwidth_limit, ..
        } = &now.target
        else {
            unreachable!()
        };
        assert_eq!(bandwidth_limit.map(|b| b.bytes()), Some(1_000_000));
        // Local targets are left alone.
        p.target = Target::Local {
            path: "/mnt".into(),
            fs_uuid: None,
        };
        assert!(matches!(
            with_bandwidth_now(&p),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}

/// What a backup's snapshots hold and what they take where they're kept.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Storage {
    pub snapshots: usize,
    /// The files of the newest snapshot.
    pub latest_bytes: u64,
    pub latest_files: u64,
    /// Every snapshot as a full copy would take.
    pub logical_bytes: u64,
    /// The space they take (`None`: not known).
    pub stored_bytes: Option<u64>,
    /// What storing each piece once saves (Vault, Borg), or sharing unchanged files
    /// between snapshots (Mirror, rsync).
    pub dedup_saved: Option<u64>,
    /// What compression saves on top.
    pub compression_saved: Option<u64>,
    /// The numbers are for a repository other backups share (a Vault): how many others.
    pub shared_with: usize,
    /// How it was measured, in words.
    pub basis: String,
}

impl AnyRepo {
    /// What the snapshots hold, what they take, and what deduplication and compression
    /// save.
    pub fn storage(&self) -> Result<Storage, RepoError> {
        let snaps = self.snapshots()?;
        let latest = snaps.iter().max_by_key(|s| s.started);
        let mut st = Storage {
            snapshots: snaps.len(),
            latest_bytes: latest.map_or(0, |s| s.stats.total_bytes),
            latest_files: latest.map_or(0, |s| s.stats.files),
            logical_bytes: snaps.iter().map(|s| s.stats.total_bytes).sum(),
            ..Default::default()
        };
        match self {
            AnyRepo::Vault { repo, owner } => {
                let index = repo.index()?;
                let (mut raw, mut stored_data) = (0u64, 0u64);
                for loc in index.map.values() {
                    if loc.entry.kind == flux_vault::format::BlobKind::Data {
                        raw += u64::from(loc.entry.raw_length);
                        stored_data += u64::from(loc.entry.length);
                    }
                }
                // A Vault is shared: its pieces are counted for every backup using it.
                let records = repo.records()?;
                let mut owners: Vec<(String, String, String)> = records
                    .iter()
                    .map(|r| {
                        (
                            r.meta.profile.clone(),
                            r.meta.host.clone(),
                            r.meta.user.clone(),
                        )
                    })
                    .collect();
                owners.sort();
                owners.dedup();
                let others = owners
                    .iter()
                    .filter(|o| (&o.0, &o.1, &o.2) != (&owner.profile, &owner.host, &owner.user))
                    .count();
                if others > 0 {
                    st.logical_bytes = records.iter().map(|r| r.meta.stats.total_bytes).sum();
                }
                st.stored_bytes = repo
                    .backend()
                    .list("data")
                    .ok()
                    .map(|l| l.iter().map(|(_, size)| size).sum());
                st.dedup_saved = Some(st.logical_bytes.saturating_sub(raw));
                st.compression_saved = Some(raw.saturating_sub(stored_data));
                st.shared_with = others;
                st.basis = "Measured from the Vault's index: each piece of data is stored once, compressed and encrypted.".into();
            }
            AnyRepo::Borg(r) => {
                let s = r.repo_stats()?;
                st.logical_bytes = s.total_size.max(st.logical_bytes);
                st.stored_bytes = Some(s.unique_csize);
                st.dedup_saved = Some(s.total_size.saturating_sub(s.unique_size));
                st.compression_saved = Some(s.unique_size.saturating_sub(s.unique_csize));
                st.basis = "Borg's own statistics (borg info).".into();
            }
            AnyRepo::Mirror(_) | AnyRepo::MirrorRemote(_) | AnyRepo::Rsync(_) => {
                // Each snapshot stored what it copied; the rest is shared with the one before.
                let copied: u64 = snaps.iter().map(|s| s.stats.copied_bytes).sum();
                st.stored_bytes = Some(copied.min(st.logical_bytes));
                st.dedup_saved = Some(st.logical_bytes.saturating_sub(copied));
                st.basis = "Unchanged files are shared between snapshots (hard links); the space is what each snapshot copied, so it's an estimate.".into();
            }
        }
        Ok(st)
    }
}
