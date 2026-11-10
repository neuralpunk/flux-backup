//! A local index of snapshot listings, so searching all snapshots and a file's history
//! don't have to read every snapshot again (decrypting Vault trees, running `borg list`,
//! walking rsync folders, fetching remote manifests). One file per snapshot in the private
//! cache folder, in the Mirror manifest format; written once (snapshots don't change),
//! dropped when the snapshot is gone. A local Mirror needs none: its manifest is one.
//!
//! The index holds file names and metadata, like Borg's own cache does: it stays in your
//! (or root's) cache folder, readable only by its owner.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;

use flux_core::{Paths, ProfileId};
use flux_fs::snapshot::SnapshotMeta;
use flux_mirror::manifest::{ManifestReader, ManifestWriter};

use crate::repo::{AnyRepo, Entries, RepoError};

fn dir(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths.cache_dir.join("index").join(id.as_str())
}

/// One file per snapshot, keyed by its id and start time (a Borg archive name can be used
/// again for a different archive).
fn file_name(snap: &SnapshotMeta) -> String {
    let safe: String = snap
        .id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}-{}.idx", snap.started.as_second())
}

/// The snapshot's entries, sorted by path: from the index when there is one, otherwise
/// from the repository (and then indexed).
pub fn entries<'a>(
    repo: &'a AnyRepo,
    paths: &Paths,
    id: &ProfileId,
    snap: &SnapshotMeta,
) -> Result<Entries<'a>, RepoError> {
    if matches!(repo, AnyRepo::Mirror(_)) {
        return repo.entries(snap);
    }
    let dir = dir(paths, id);
    let path = dir.join(file_name(snap));
    // The index streams from its file: a search of every snapshot runs on every core,
    // and a whole file list held by each (as this used to give) is gigabytes at once.
    if !path.exists() {
        match write(&dir, &path, repo.entries(snap)?) {
            Ok(()) => {}
            // What couldn't be read from the backup is the caller's to know.
            Err(Indexing::Repo(e)) => return Err(e),
            // No room for an index, say: the backup's own list, as it comes.
            Err(Indexing::Io(e)) => {
                tracing::debug!("not indexing {}: {e}", snap.id);
                return repo.entries(snap);
            }
        }
    }
    match ManifestReader::open(&path) {
        Ok(r) => {
            let gone = path.clone();
            Ok(Box::new(r.map(move |e| {
                e.map_err(|e| {
                    // Damaged: made anew the next time it's wanted.
                    tracing::debug!("dropping damaged index {}: {e}", gone.display());
                    let _ = std::fs::remove_file(&gone);
                    RepoError::Mirror(e.into())
                })
            })))
        }
        Err(e) => {
            tracing::debug!("dropping damaged index {}: {e}", path.display());
            let _ = std::fs::remove_file(&path);
            repo.entries(snap)
        }
    }
}

enum Indexing {
    Repo(RepoError),
    Io(std::io::Error),
}

/// Write `entries` to the index file `path` as they come (none of them kept).
fn write(
    dir: &std::path::Path,
    path: &std::path::Path,
    entries: Entries<'_>,
) -> Result<(), Indexing> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(Indexing::Io)?;
    let tmp = path.with_extension(format!(
        "tmp{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let result = (|| {
        let io = |e| Indexing::Io(std::io::Error::other(e));
        let mut w = ManifestWriter::create(&tmp).map_err(io)?;
        for e in entries {
            w.write(&e.map_err(Indexing::Repo)?).map_err(io)?;
        }
        w.finish().map_err(io)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(Indexing::Io)?;
        std::fs::rename(&tmp, path).map_err(Indexing::Io)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Remove the index files of snapshots that no longer exist.
pub fn prune(paths: &Paths, id: &ProfileId, snaps: &[SnapshotMeta]) {
    let Ok(read) = std::fs::read_dir(dir(paths, id)) else {
        return;
    };
    let keep: std::collections::HashSet<String> = snaps.iter().map(file_name).collect();
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !keep.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Forget a profile's index (it was deleted).
pub fn remove(paths: &Paths, id: &ProfileId) {
    let _ = std::fs::remove_dir_all(dir(paths, id));
}
