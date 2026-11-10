//! Flux Vault engine: deduplicated, compressed, encrypted snapshots in a content-addressed
//! repository. See `docs/VAULT_FORMAT.md` for the on-disk format.
//!
//! * [`repo::VaultRepo`]: open/create a repository, keys, snapshots, trees
//! * [`backup::backup`]: take a snapshot
//! * [`VaultSource`]: restore through `flux_fs::restore`
//! * [`check::check`]: verify structure and (optionally) every byte
//! * [`gc::prune`]: apply retention and reclaim space

#![deny(clippy::unwrap_used)]

pub mod backend;
pub mod backup;
pub mod check;
pub mod crypto;
pub mod format;
pub mod gc;
pub mod pack;
pub mod replicate;
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
    #[error("this Vault's password works only together with its unlock file, which wasn't found")]
    UnlockFileNeeded,
    #[error("this Vault is not encrypted; it has no passwords")]
    NotEncrypted,
    #[error("an empty password protects nothing; choose one")]
    EmptyPassword,
    #[error("no key {0:?} in this Vault")]
    NoSuchKey(String),
    #[error("refusing to remove the last key: the Vault would become unreadable")]
    LastKey,
    #[error("another Flux job is using the Vault at {place}{}", held(held_by))]
    Locked {
        place: String,
        /// Who holds it, when their lock file says ("alice@laptop, process 42, since …").
        held_by: Vec<String>,
    },
    #[error("Vault is damaged: {0}")]
    Corrupt(String),
    #[error("snapshot {0:?} not found")]
    SnapshotNotFound(String),
    #[error(
        "snapshot {0} can't be read ({1}), so nothing was deleted: its data would be taken for unused. Try again; if it stays unreadable, delete that snapshot first"
    )]
    UnreadableSnapshot(String, String),
    #[error("{0:?} matches {1} snapshots; be more specific")]
    Ambiguous(String, usize),
    #[error("cancelled; data stored so far is reused by the next run")]
    Cancelled,
    #[error("stopped at the first error (continue_on_errors is off): {0}")]
    Aborted(String),
    #[error(
        "couldn't store data in the Vault ({0}); no snapshot was made, and what was stored is reused by the next run"
    )]
    Store(String),
}

fn held(by: &[String]) -> String {
    if by.is_empty() {
        String::new()
    } else {
        format!(
            " ({}). If that job is no longer running, `flux unlock` removes its lock",
            by.join("; ")
        )
    }
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
    entries: Vec<(Entry, format::Content)>,
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
        use std::os::unix::fs::FileExt;
        let i = *self
            .content
            .get(&e.path)
            .ok_or_else(|| io::Error::other("not in snapshot"))?;
        let index = self.repo.index().map_err(io::Error::other)?;
        let content = &self.entries[i].1;
        // The data goes into the runs between the holes; the holes are never written, so
        // they stay holes (the file is new and empty), and `set_len` keeps a trailing one.
        let mut runs = content.data_runs(e.size).into_iter();
        let mut run = runs.next().unwrap_or((0, 0));
        let mut n = 0u64;
        for id in &content.blobs {
            if progress.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let loc = index.get(id).ok_or_else(|| {
                io::Error::other(format!("data blob {} is missing", crypto::hex_id(id)))
            })?;
            let data = pack::read_blob(self.repo.backend(), &self.repo.crypto, loc)
                .map_err(io::Error::other)?;
            let mut rest = &data[..];
            while !rest.is_empty() {
                if run.1 == 0 {
                    run = runs.next().ok_or_else(|| {
                        io::Error::other(format!(
                            "{}: more data than the file holds",
                            e.rel_path().display()
                        ))
                    })?;
                }
                let take = rest.len().min(run.1 as usize);
                out.write_all_at(&rest[..take], run.0)?;
                run = (run.0 + take as u64, run.1 - take as u64);
                rest = &rest[take..];
            }
            n += data.len() as u64;
            progress
                .bytes_done
                .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let holes = content.hole_bytes();
        if holes > 0 {
            out.set_len(e.size)?;
            progress
                .bytes_done
                .fetch_add(holes, std::sync::atomic::Ordering::Relaxed);
        }
        out.sync_all()?;
        Ok(n + holes)
    }
}

/// The entries of a Vault snapshot, sorted by path (for diff, find and history).
pub fn entries_of(repo: &VaultRepo, snapshot_id: &str) -> Result<Vec<Entry>, VaultError> {
    let record = repo.read_record(snapshot_id)?;
    Ok(repo.flatten(&record)?.into_iter().map(|(e, _)| e).collect())
}
