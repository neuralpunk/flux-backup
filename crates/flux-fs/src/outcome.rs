//! Results of a backup run, shared by all engines.

use serde::Serialize;

use crate::snapshot::{FileError, SnapshotMeta};

/// What a backup will do / did, by entry counts and bytes. For Mirror, "copy" means copied
/// and "link" means hard-linked. For Vault, "copy" means read and stored, and "link" means
/// reused from the previous snapshot without reading.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanStats {
    pub entries: u64,
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub specials: u64,
    pub total_bytes: u64,
    pub new: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub deleted: u64,
    pub copy_files: u64,
    pub copy_bytes: u64,
    pub link_files: u64,
}

impl PlanStats {
    pub fn has_changes(&self) -> bool {
        self.new + self.changed + self.deleted > 0
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BackupOutcome {
    Created {
        snapshot: Box<SnapshotMeta>,
        /// Snapshots removed by retention or to make room.
        pruned: Vec<String>,
    },
    /// Nothing changed since `previous`; no snapshot was made.
    Unchanged {
        previous: String,
        pruned: Vec<String>,
    },
    DryRun {
        stats: PlanStats,
        errors: Vec<FileError>,
    },
}

/// Result of verifying a repository, in engine-neutral terms.
#[derive(Debug, Default, Serialize)]
pub struct CheckSummary {
    pub snapshots: u64,
    /// Items verified (files for Mirror, blobs for Vault).
    pub checked: u64,
    pub bytes_read: u64,
    /// Problems that make data unrestorable.
    pub errors: Vec<String>,
    /// Harmless findings.
    pub warnings: Vec<String>,
    /// Damage that was repaired from repair data (Vaults that keep it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repaired: Vec<String>,
    /// Of `errors`, how many are damage the repair data can rebuild (it wasn't asked to).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub repairable: u64,
    /// Stored files given repair data they lacked.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub parity_added: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
