//! Snapshot metadata, shared by every engine.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    Complete,
    /// Finished, but some paths could not be backed up (see `errors`).
    CompleteWithErrors,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub entries: u64,
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub specials: u64,
    /// Sum of all regular file sizes in the snapshot.
    pub total_bytes: u64,
    pub new: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub deleted: u64,
    /// Bytes actually copied (new + changed files).
    pub copied_bytes: u64,
    pub copied_files: u64,
    pub linked_files: u64,
    /// Bytes added to the repository (Vault: after dedup and compression).
    #[serde(default)]
    pub stored_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileError {
    pub path: String,
    pub message: String,
}

/// Contents of `meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub format: u32,
    pub id: String,
    pub profile: String,
    pub host: String,
    pub user: String,
    pub started: Timestamp,
    pub finished: Timestamp,
    pub status: SnapshotStatus,
    /// User-given name; named snapshots are never pruned automatically.
    #[serde(default)]
    pub name: Option<String>,
    /// Pinned snapshots are never pruned, whatever the retention policy says.
    #[serde(default)]
    pub pinned: bool,
    pub sources: Vec<PathBuf>,
    pub stats: SnapshotStats,
    /// First errors encountered (capped); `errors_total` has the full count.
    #[serde(default)]
    pub errors: Vec<FileError>,
    #[serde(default)]
    pub errors_total: u64,
    pub flux_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FindError {
    #[error("snapshot {0:?} not found")]
    NotFound(String),
    #[error("{0:?} matches {1} snapshots; be more specific")]
    Ambiguous(String, usize),
}

/// Pick a snapshot from `all` (oldest first) by `latest`, exact id, unique id prefix, or
/// name.
pub fn find_snapshot(mut all: Vec<SnapshotMeta>, spec: &str) -> Result<SnapshotMeta, FindError> {
    if spec == "latest" {
        return all.pop().ok_or_else(|| FindError::NotFound(spec.into()));
    }
    if let Some(i) = all.iter().position(|s| s.id == spec) {
        return Ok(all.swap_remove(i));
    }
    let mut m: Vec<_> = all
        .into_iter()
        .filter(|s| s.id.starts_with(spec) || s.name.as_deref() == Some(spec))
        .collect();
    match m.len() {
        1 => Ok(m.pop().expect("one match")),
        0 => Err(FindError::NotFound(spec.into())),
        n => Err(FindError::Ambiguous(spec.into(), n)),
    }
}
