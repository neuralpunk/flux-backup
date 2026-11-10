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
    /// Sources whose drive wasn't connected when this snapshot was taken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources_missing: Vec<SourceMissing>,
    /// A note in the user's words ("before the upgrade"). Unlike a name, it never keeps a
    /// snapshot from being pruned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Labels for finding snapshots (`taxes`, `release`). They never affect retention.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Longest note kept, in characters.
/// The version of `meta.json` this Flux writes and the newest it reads.
pub const META_FORMAT: u32 = 1;

impl SnapshotMeta {
    /// Refuse a snapshot written by a newer Flux (its `format` is newer than this one's).
    pub fn check_format(&self) -> Result<(), String> {
        if self.format > META_FORMAT {
            return Err(format!(
                "snapshot {} was written by a newer Flux (format {}, this one reads up to {META_FORMAT}); update Flux Backup",
                self.id, self.format
            ));
        }
        Ok(())
    }
}

pub const NOTE_MAX: usize = 2000;
/// Longest tag, in characters, and most tags on a snapshot.
pub const TAG_MAX: usize = 40;
pub const TAGS_MAX: usize = 20;

/// A note as stored: trimmed, at most [`NOTE_MAX`] characters; empty is no note.
pub fn clean_note(note: Option<&str>) -> Option<String> {
    let t = note?.trim();
    (!t.is_empty()).then(|| t.chars().take(NOTE_MAX).collect())
}

/// Tags as stored: trimmed, inner whitespace collapsed, at most [`TAG_MAX`] characters,
/// no duplicates (ignoring case; the first spelling wins), at most [`TAGS_MAX`].
pub fn clean_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t: String = t
            .as_ref()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(TAG_MAX)
            .collect();
        if !t.is_empty() && !out.iter().any(|o| o.to_lowercase() == t.to_lowercase()) {
            out.push(t);
        }
    }
    out.truncate(TAGS_MAX);
    out
}

/// A change to a snapshot's user-given details. `None` fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotChange {
    pub name: Option<Option<String>>,
    pub pinned: Option<bool>,
    pub note: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
}

impl SnapshotChange {
    pub fn apply(&self, m: &mut SnapshotMeta) {
        if let Some(n) = &self.name {
            m.name = n.clone();
        }
        if let Some(p) = self.pinned {
            m.pinned = p;
        }
        if let Some(n) = &self.note {
            m.note = clean_note(n.as_deref());
        }
        if let Some(t) = &self.tags {
            m.tags = clean_tags(t);
        }
    }
}

#[cfg(test)]
mod note_tests {
    use super::*;

    #[test]
    fn notes_and_tags_are_cleaned() {
        assert_eq!(
            clean_note(Some("  before the upgrade \n")).as_deref(),
            Some("before the upgrade")
        );
        assert_eq!(clean_note(Some("   ")), None);
        assert_eq!(clean_note(None), None);
        assert_eq!(clean_note(Some(&"x".repeat(5000))).unwrap().len(), NOTE_MAX);
        assert_eq!(
            clean_tags([" taxes ", "Taxes", "tax   year 2025", "", "release"]),
            ["taxes", "tax year 2025", "release"]
        );
        assert_eq!(clean_tags((0..30).map(|i| format!("t{i}"))).len(), TAGS_MAX);
        assert_eq!(clean_tags([&"y".repeat(99)])[0].len(), TAG_MAX);
    }
}

/// A source whose drive wasn't connected for a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMissing {
    pub path: PathBuf,
    /// Its files were kept from this earlier snapshot (Mirror and Vault); `None`: this
    /// snapshot doesn't have them (earlier ones still do).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_from: Option<String>,
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
