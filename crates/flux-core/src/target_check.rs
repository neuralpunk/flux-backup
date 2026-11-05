//! Safety checks run before writing to a local target.
//!
//! The classic backup disaster: the USB drive isn't plugged in, `/run/media/me/Backup` is
//! an empty directory on the root filesystem, and the backup fills `/` instead. Flux refuses
//! to write in that situation.

use std::path::{Path, PathBuf};

use crate::sys;

/// Mount roots where removable or network drives are normally attached.
const REMOVABLE_ROOTS: &[&str] = &["/run/media", "/media", "/mnt"];

#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    #[error("backup target {0} does not exist (is the drive connected and mounted?)")]
    Missing(PathBuf),
    #[error("backup target {0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("the drive with filesystem UUID {uuid} is not mounted at {path}")]
    WrongFilesystem { path: PathBuf, uuid: String },
    #[error(
        "backup target {0} is not on a mounted drive; it is on the system disk. \
         Mount the drive, or set target.fs_uuid so Flux can tell which drive belongs here"
    )]
    LooksUnmounted(PathBuf),
}

/// Check that `path` is a usable local target.
pub fn check_local_target(path: &Path, fs_uuid: Option<&str>) -> Result<(), TargetError> {
    let meta = std::fs::metadata(path).map_err(|_| TargetError::Missing(path.to_owned()))?;
    if !meta.is_dir() {
        return Err(TargetError::NotADirectory(path.to_owned()));
    }
    if let Some(uuid) = fs_uuid {
        if !sys::path_is_on_fs_uuid(path, uuid) {
            return Err(TargetError::WrongFilesystem {
                path: path.to_owned(),
                uuid: uuid.to_owned(),
            });
        }
        return Ok(());
    }
    if looks_unmounted(path) {
        return Err(TargetError::LooksUnmounted(path.to_owned()));
    }
    Ok(())
}

/// True if `path` is under a removable-media root but its filesystem is mounted *above*
/// that root (typically `/`): the drive that should be there is missing.
fn looks_unmounted(path: &Path) -> bool {
    let Ok(canon) = std::fs::canonicalize(path) else {
        return false;
    };
    let Some(root) = REMOVABLE_ROOTS
        .iter()
        .map(Path::new)
        .find(|r| canon.starts_with(r))
    else {
        return false;
    };
    match sys::mount_of(&canon) {
        Some(m) => !m.mount_point.starts_with(root) || m.mount_point == root,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_plain_dirs() {
        let t = tempfile::tempdir().unwrap();
        assert!(check_local_target(t.path(), None).is_ok());
        let f = t.path().join("file");
        std::fs::write(&f, "").unwrap();
        assert!(matches!(
            check_local_target(&f, None),
            Err(TargetError::NotADirectory(_))
        ));
        assert!(matches!(
            check_local_target(&t.path().join("nope"), None),
            Err(TargetError::Missing(_))
        ));
        assert!(matches!(
            check_local_target(t.path(), Some("0000-not-a-uuid")),
            Err(TargetError::WrongFilesystem { .. })
        ));
    }
}
