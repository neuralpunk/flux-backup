//! Verify a Mirror repository: every manifest decodes and every file it lists exists in
//! the data tree with the right size. With `read_data`, every file is read back (and
//! compared with its hash when one was recorded).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;

use flux_core::progress::{Phase, Progress};
use flux_fs::outcome::CheckSummary;

use crate::MirrorError;
use crate::manifest::{Kind, ManifestReader};
use crate::repo::MirrorRepo;
use flux_core::sync::Locked;

pub fn check(
    repo: &MirrorRepo,
    read_data: bool,
    progress: &Progress,
) -> Result<CheckSummary, MirrorError> {
    progress.set_phase(Phase::Scanning);
    let snaps = repo.snapshots()?;
    let errors = Mutex::new(Vec::new());
    let checked = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    // Files read so far: (device, inode, the hash expected of it).
    type Read = (u64, u64, Option<[u8; 32]>);
    let read: Mutex<std::collections::HashSet<Read>> = Mutex::default();
    for s in &snaps {
        let entries = match ManifestReader::read_all(&repo.manifest_path(&s.id)) {
            Ok(e) => e,
            Err(e) => {
                errors.locked().push(format!("snapshot {}: {e}", s.id));
                continue;
            }
        };
        let data = repo.data_dir(&s.id);
        entries
            .par_iter()
            .filter(|e| matches!(e.kind, Kind::File | Kind::Symlink | Kind::Dir))
            .for_each(|e| {
                if progress.is_cancelled() {
                    return;
                }
                let p = data.join(e.rel_path());
                let problem = match (fs::symlink_metadata(&p), e.kind) {
                    (Err(err), _) => Some(format!("missing ({err})")),
                    (Ok(m), Kind::File) if !m.is_file() || m.len() != e.size => {
                        Some(format!("size {} instead of {}", m.len(), e.size))
                    }
                    // Read once: an unchanged file is the same file (a hard link) in
                    // every snapshot that has it, and one read checks it for them all.
                    // Once per snapshot, a check reads the backup as many times over as
                    // it has snapshots.
                    (Ok(m), Kind::File)
                        if read_data && read.locked().insert((m.dev(), m.ino(), e.hash)) =>
                    {
                        match flux_fs::fsops::hash_file(&p) {
                            Ok(h) => {
                                bytes.fetch_add(e.size, Relaxed);
                                e.hash
                                    .filter(|want| *want != h)
                                    .map(|_| "content differs from its recorded hash".to_owned())
                            }
                            Err(err) => Some(format!("unreadable ({err})")),
                        }
                    }
                    (Ok(m), Kind::Symlink) if !m.file_type().is_symlink() => {
                        Some("not a symlink".into())
                    }
                    (Ok(m), Kind::Dir) if !m.is_dir() => Some("not a directory".into()),
                    _ => None,
                };
                if let Some(msg) = problem {
                    errors.locked().push(format!(
                        "snapshot {}: {}: {msg}",
                        s.id,
                        e.abs_path().display()
                    ));
                }
                checked.fetch_add(1, Relaxed);
            });
    }
    if progress.is_cancelled() {
        return Err(MirrorError::Cancelled);
    }
    let mut errors = errors.into_inner().expect("errors");
    errors.sort();
    let mut warnings = Vec::new();
    if repo.partial_dir().exists() {
        warnings.push(
            "an interrupted backup left a partial snapshot; the next backup resumes it".into(),
        );
    }
    progress.set_phase(Phase::Done);
    Ok(CheckSummary {
        snapshots: snaps.len() as u64,
        checked: checked.into_inner(),
        bytes_read: bytes.into_inner(),
        errors,
        warnings,
        ..Default::default()
    })
}
