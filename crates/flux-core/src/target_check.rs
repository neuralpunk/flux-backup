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

/// The filesystem `path` is on (`ext4`, `vfat`, `exfat`, `ntfs3`…), as the kernel mounted
/// it. A drive mounted through FUSE (`fuseblk`, as ntfs-3g and exfat-fuse do) is looked up
/// in udev's database, which says what the partition holds.
pub fn filesystem_of(path: &Path) -> Option<String> {
    let m = sys::mount_of(path)?;
    if m.fs_type != "fuseblk" {
        return Some(m.fs_type);
    }
    Some(udev_fs_type(Path::new(&m.source)).unwrap_or(m.fs_type))
}

/// `ID_FS_TYPE` for block device `dev`, from `/run/udev/data/b<major>:<minor>`.
fn udev_fs_type(dev: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let rdev = std::fs::metadata(dev).ok()?.rdev();
    let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
    let text = std::fs::read_to_string(format!("/run/udev/data/b{major}:{minor}")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("E:ID_FS_TYPE="))
        .map(str::to_owned)
}

/// What a drive's format means for a backup that goes there, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsFit {
    /// It costs space or loses something every backup (plain folders on FAT or exFAT),
    /// not only permissions.
    pub severe: bool,
    /// What's wrong.
    pub text: String,
    /// What to do.
    pub hint: String,
}

impl FsFit {
    pub fn sentence(&self) -> String {
        format!("{} {}", self.text, self.hint)
    }
}

/// The name people know a filesystem by, for the ones that matter here.
fn known_name(fs: &str) -> Option<&'static str> {
    Some(match fs {
        "vfat" | "msdos" | "fat" => "FAT32",
        "exfat" => "exFAT",
        "ntfs" | "ntfs3" => "NTFS",
        _ => return None,
    })
}

/// What filesystem `fs` can't do that backups made by `engine` need: plain folders
/// (Mirror, rsync) hard-link unchanged files from the snapshot before and keep links,
/// owners and permissions as they are; a Vault (or Borg) keeps all of that in its own
/// files, so any filesystem suits it.
pub fn filesystem_fit(fs: &str, engine: crate::profile::Engine) -> Option<FsFit> {
    use crate::profile::Engine;
    if !matches!(engine, Engine::Mirror | Engine::Rsync) {
        return None;
    }
    let name = known_name(fs)?;
    let hint = "A Vault backup suits it (it keeps all of that in its own files); or format the drive as ext4, which erases it.".to_owned();
    Some(match name {
        "FAT32" => FsFit {
            severe: true,
            text: "This drive is formatted FAT32, which can't keep hard links, links or Linux permissions: every snapshot is a full copy, links and permissions are lost, and no file of 4 GB or more fits.".into(),
            hint,
        },
        "exFAT" => FsFit {
            severe: true,
            text: "This drive is formatted exFAT, which can't keep hard links, links or Linux permissions: every snapshot is a full copy, and links and permissions are lost.".into(),
            hint,
        },
        _ => FsFit {
            severe: false,
            text: "This drive is formatted NTFS, which doesn't keep Linux owners and permissions: files restored from it get default ones.".into(),
            hint,
        },
    })
}

/// [`filesystem_fit`] for where profile `p` keeps its backups, when that's a local folder
/// whose drive is mounted.
pub fn filesystem_fit_of(p: &crate::Profile) -> Option<FsFit> {
    let crate::Target::Local { path, .. } = &p.target else {
        return None;
    };
    if path.as_os_str().is_empty() {
        return None;
    }
    filesystem_fit(&filesystem_of(path)?, p.engine)
}

/// The words for a write that failed because a file is bigger than the drive's filesystem
/// can hold (`EFBIG`), or `None` for another error.
pub fn too_big_for_drive(err: &std::io::Error, fs: Option<&str>) -> Option<String> {
    const EFBIG: i32 = 27;
    if err.raw_os_error() != Some(EFBIG) {
        return None;
    }
    Some(match fs.and_then(known_name) {
        Some("FAT32") => "too big for this drive: FAT32 holds no file of 4 GB or more (a Vault backup splits files up; or format the drive as ext4)".into(),
        Some(name) => format!("too big for this drive's filesystem ({name})"),
        None => "too big for this drive's filesystem".into(),
    })
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

    #[test]
    fn plain_folders_on_fat_and_exfat_are_told_vaults_are_not() {
        use crate::profile::Engine;
        let fat = filesystem_fit("vfat", Engine::Mirror).unwrap();
        assert!(fat.severe && fat.text.contains("4 GB"));
        assert!(filesystem_fit("exfat", Engine::Rsync).unwrap().severe);
        let ntfs = filesystem_fit("ntfs3", Engine::Mirror).unwrap();
        assert!(!ntfs.severe && ntfs.text.contains("permissions"));
        assert_eq!(filesystem_fit("vfat", Engine::Vault), None);
        assert_eq!(filesystem_fit("exfat", Engine::Borg), None);
        assert_eq!(filesystem_fit("ext4", Engine::Mirror), None);
        assert_eq!(filesystem_fit("btrfs", Engine::Rsync), None);
    }

    #[test]
    fn a_file_too_big_for_fat32_is_named_as_such() {
        let e = std::io::Error::from_raw_os_error(27);
        assert!(
            too_big_for_drive(&e, Some("vfat"))
                .unwrap()
                .contains("4 GB")
        );
        assert!(too_big_for_drive(&e, None).is_some());
        let other = std::io::Error::from_raw_os_error(28);
        assert_eq!(too_big_for_drive(&other, Some("vfat")), None);
    }

    #[test]
    fn the_filesystem_of_a_folder_is_known() {
        let t = tempfile::tempdir().unwrap();
        assert!(filesystem_of(t.path()).is_some_and(|f| !f.is_empty()));
    }
}
