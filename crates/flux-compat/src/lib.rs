//! Compatibility mode: backups stored in the native format of a standard tool, so they
//! stay usable without Flux.
//!
//! * [`rsync`]: every snapshot is a plain folder written by the system `rsync` with
//!   `--link-dest` (unchanged files are hard links to the previous snapshot).
//! * [`borg`]: a standard Borg 1.x repository written by the system `borg`; every
//!   snapshot is a Borg archive.
//!
//! Next to every repository Flux keeps a recovery kit ([`kit`]): `FLUX-RECOVERY.txt`
//! with step-by-step commands for this profile, and `flux-backup.sh`, which runs the same
//! backup with the standard tool only. Flux's own files (snapshot metadata, cached
//! listings) are conveniences; nothing in them is needed to get data back.

#![deny(clippy::unwrap_used)]

pub mod backintime;
pub mod borg;
pub mod export;
pub mod filter;
pub mod kit;
pub mod listing;
pub mod prune;
pub mod rsync;
pub mod store;
pub mod tools;

use std::path::PathBuf;

pub use store::Store;

/// Snapshot errors listed in a snapshot's metadata (the full count is kept separately).
pub(crate) const MAX_STORED_ERRORS: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum CompatError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Remote(String),
    /// The standard tool is missing or too old.
    #[error("{0}")]
    Tool(String),
    /// The standard tool ran and failed.
    #[error("{0}")]
    Failed(String),
    #[error("no backups at {0} yet")]
    NoRepository(String),
    #[error("{0} is not a {1} repository made by Flux")]
    Foreign(String, &'static str),
    #[error("snapshot {0:?} not found")]
    SnapshotNotFound(String),
    #[error("{0:?} matches {1} snapshots; be more specific")]
    Ambiguous(String, usize),
    #[error("{0} is locked by another Flux process")]
    Locked(String),
    #[error("none of the source paths exist")]
    NoSources,
    #[error(transparent)]
    Filter(#[from] flux_core::filter::FilterError),
    #[error(transparent)]
    Target(#[from] flux_core::target_check::TargetError),
    #[error("{0}")]
    Corrupt(String),
    #[error("the Borg passphrase is wrong")]
    WrongPassphrase,
    #[error("this Borg repository is encrypted and needs its passphrase")]
    PassphraseRequired,
    #[error("cancelled")]
    Cancelled,
}

impl CompatError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CompatError::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn remote(e: impl std::fmt::Display) -> Self {
        CompatError::Remote(e.to_string())
    }
}

impl From<flux_fs::snapshot::FindError> for CompatError {
    fn from(e: flux_fs::snapshot::FindError) -> Self {
        match e {
            flux_fs::snapshot::FindError::NotFound(s) => CompatError::SnapshotNotFound(s),
            flux_fs::snapshot::FindError::Ambiguous(s, n) => CompatError::Ambiguous(s, n),
        }
    }
}

impl From<CompatError> for std::io::Error {
    fn from(e: CompatError) -> Self {
        match e {
            CompatError::Io { source, .. } => source,
            other => std::io::Error::other(other),
        }
    }
}

/// A fresh snapshot id for `now` in local time (`2026-11-07_18-30-00`), made unique by
/// `taken`.
pub(crate) fn new_snapshot_id(now: jiff::Timestamp, taken: impl Fn(&str) -> bool) -> String {
    let zoned = now.to_zoned(jiff::tz::TimeZone::system());
    let base = zoned.strftime("%Y-%m-%d_%H-%M-%S").to_string();
    let mut id = base.clone();
    let mut n = 2;
    while taken(&id) {
        id = format!("{base}_{n}");
        n += 1;
    }
    id
}

/// The time encoded in a snapshot id (local time), for snapshots made without Flux.
pub(crate) fn time_from_id(id: &str) -> Option<jiff::Timestamp> {
    let base = id.get(..19)?;
    let dt = jiff::civil::DateTime::strptime("%Y-%m-%d_%H-%M-%S", base).ok()?;
    dt.to_zoned(jiff::tz::TimeZone::system())
        .ok()
        .map(|z| z.timestamp())
}

/// `p` as it may be written into a script, a guide or another tool's settings: its name
/// and exclusions go into comment lines and here-documents, where a line break starts a
/// new line of the file, and so a command. No profile Flux makes has one; settings read
/// from a drive, or imported, might. In the name a control character becomes a space; in
/// a pattern it becomes `?` (any one character), which still matches a file name that
/// really has one.
pub(crate) fn script_safe(p: &flux_core::Profile) -> flux_core::Profile {
    let mut p = p.clone();
    let swap = |s: &str, with: char| -> String {
        s.chars()
            .map(|c| if c.is_control() { with } else { c })
            .collect()
    };
    p.name = swap(&p.name, ' ');
    for list in [&mut p.sources.exclude, &mut p.sources.exclude_if_present] {
        for pattern in list.iter_mut() {
            *pattern = swap(pattern, '?');
        }
    }
    p
}

/// A here-document end marker that no line of `body` is: a line of the body equal to the
/// marker would end the document early, and the lines after it would run as commands.
pub(crate) fn heredoc_tag(base: &str, body: &str) -> String {
    let mut tag = base.to_owned();
    while body.lines().any(|l| l == tag) {
        tag.push('_');
    }
    tag
}

/// A name that can be used as one path component of a snapshot.
pub(crate) fn valid_snapshot_name(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('.') && !id.contains('/') && id != ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_ids_round_trip_to_times() {
        let now = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        let id = new_snapshot_id(now, |_| false);
        assert_eq!(time_from_id(&id), Some(now));
        let taken = id.clone();
        let second = new_snapshot_id(now, |s| s == taken);
        assert_eq!(second, format!("{id}_2"));
        assert_eq!(time_from_id(&second), Some(now));
        assert_eq!(time_from_id("not-a-date"), None);
        assert!(!valid_snapshot_name(".partial"));
        assert!(!valid_snapshot_name("a/b"));
        assert!(valid_snapshot_name(&id));
    }
}
