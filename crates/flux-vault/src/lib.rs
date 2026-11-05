//! Flux Vault engine: deduplicated, compressed, encrypted snapshots in a content-addressed
//! repository. See `docs/VAULT_FORMAT.md` for the on-disk format.
//!
//! * [`repo::VaultRepo`]: open/create a repository, keys, snapshots, trees
//! * [`backup::backup`]: take a snapshot
//! * [`VaultSource`]: restore through `flux_fs::restore`
//! * [`check::check`]: verify structure and (optionally) every byte
//! * [`gc::prune`]: apply retention and reclaim space

pub mod backend;
pub mod backup;
pub mod check;
pub mod crypto;
pub mod format;
pub mod gc;
pub mod pack;
pub mod repo;

use std::io;
use std::path::{Path, PathBuf};

use flux_core::progress::Progress;
use flux_fs::entry::Entry;
use flux_fs::fsops::Copier;

pub use backend::{Backend, LocalBackend};
pub use repo::{Owner, VaultRepo};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error(transparent)]
    Scan(#[from] flux_fs::scan::ProfileScanError),
    #[error("no Vault found at {0}")]
    NoRepository(String),
    #[error("a Vault already exists at {0}")]
    AlreadyExists(String),
    #[error("this Vault is encrypted; a password is needed")]
    PasswordRequired,
    #[error("wrong password (no key of this Vault opens with it)")]
    WrongPassword,
    #[error("this Vault is not encrypted; it has no passwords")]
    NotEncrypted,
    #[error("no key {0:?} in this Vault")]
    NoSuchKey(String),
    #[error("refusing to remove the last key: the Vault would become unreadable")]
    LastKey,
    #[error("another Flux job is using the Vault at {0}")]
    Locked(String),
    #[error("Vault is damaged: {0}")]
    Corrupt(String),
    #[error("snapshot {0:?} not found")]
    SnapshotNotFound(String),
    #[error("{0:?} matches {1} snapshots; be more specific")]
    Ambiguous(String, usize),
    #[error("cancelled; data stored so far is reused by the next run")]
    Cancelled,
    #[error("stopped at the first error (continue_on_errors is off): {0}")]
    Aborted(String),
}

impl VaultError {
    pub(crate) fn io(path: &str, source: io::Error) -> Self {
        VaultError::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// Where the Vault for a local target lives.
pub fn local_root(target: &Path) -> PathBuf {
    target.join("flux-vault")
}

/// A Vault snapshot as a restore source.
pub struct VaultSource<'a> {
    repo: &'a VaultRepo,
    entries: Vec<(Entry, Vec<crypto::BlobId>)>,
    content: std::collections::HashMap<Vec<u8>, usize>,
}

impl<'a> VaultSource<'a> {
    pub fn new(repo: &'a VaultRepo, snapshot_id: &str) -> Result<Self, VaultError> {
        let record = repo.read_record(snapshot_id)?;
        let entries = repo.flatten(&record)?;
        let content = entries
            .iter()
            .enumerate()
            .map(|(i, (e, _))| (e.path.clone(), i))
            .collect();
        Ok(Self {
            repo,
            entries,
            content,
        })
    }
}

impl flux_fs::restore::RestoreSource for VaultSource<'_> {
    fn entries(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<Entry>> + '_>> {
        Ok(Box::new(self.entries.iter().map(|(e, _)| Ok(e.clone()))))
    }

    fn write_file(
        &self,
        e: &Entry,
        out: &std::fs::File,
        _copier: &Copier,
        progress: &Progress,
    ) -> io::Result<u64> {
        use std::io::Write;
        let mut out = out;
        let i = *self
            .content
            .get(&e.path)
            .ok_or_else(|| io::Error::other("not in snapshot"))?;
        let index = self.repo.index().map_err(io::Error::other)?;
        let mut n = 0u64;
        for id in &self.entries[i].1 {
            if progress.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let loc = index.get(id).ok_or_else(|| {
                io::Error::other(format!("data blob {} is missing", crypto::hex_id(id)))
            })?;
            let data = pack::read_blob(self.repo.backend(), &self.repo.crypto, loc)
                .map_err(io::Error::other)?;
            out.write_all(&data)?;
            n += data.len() as u64;
            progress
                .bytes_done
                .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        out.sync_all()?;
        Ok(n)
    }
}

/// The entries of a Vault snapshot, sorted by path (for diff, find and history).
pub fn entries_of(repo: &VaultRepo, snapshot_id: &str) -> Result<Vec<Entry>, VaultError> {
    let record = repo.read_record(snapshot_id)?;
    Ok(repo.flatten(&record)?.into_iter().map(|(e, _)| e).collect())
}
