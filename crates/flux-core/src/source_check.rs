//! Before a backup reads its sources: is every source's drive there?
//!
//! The source-side twin of [`crate::target_check`]. Imagine backing up an external disk
//! mounted at `/mnt/photos`. Unplugged, `/mnt/photos` is an empty folder on the system
//! disk (or gone, for drives the desktop mounts under `/run/media`), and a backup that
//! reads it would record that every photo was deleted. A source counts as missing when:
//!
//! * the profile recorded its drive's filesystem UUID (`sources.volumes`) and the path
//!   isn't on that filesystem now, or
//! * it lies under `/run/media`, `/media` or `/mnt` but is on the system disk, or doesn't
//!   exist: the drive that belongs there isn't mounted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::profile::{Profile, SourceVolume, WhenMissing};
use crate::sys::{self, Mounts};

/// Removable-drive roots (the same as for targets).
const REMOVABLE_ROOTS: &[&str] = &["/run/media", "/media", "/mnt"];

/// How long `wait` waits by default.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(3600);

/// A source whose drive isn't there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingSource {
    pub path: PathBuf,
    /// In words: "the drive with filesystem UUID … isn't connected".
    pub why: String,
    pub when_missing: WhenMissing,
    /// With `wait`: how long at most.
    #[serde(skip)]
    pub wait_for: Duration,
}

/// The volume recorded for `path`, if any.
pub fn volume_for<'a>(profile: &'a Profile, path: &Path) -> Option<&'a SourceVolume> {
    profile.sources.volumes.iter().find(|v| v.path == path)
}

/// Every source of `profile` whose drive isn't there now.
pub fn missing_sources(profile: &Profile) -> Vec<MissingSource> {
    missing_sources_in(profile, &Mounts::current())
}

/// [`missing_sources`] with these mounts.
pub fn missing_sources_in(profile: &Profile, mounts: &Mounts) -> Vec<MissingSource> {
    profile
        .sources
        .paths
        .iter()
        .filter_map(|path| {
            let vol = volume_for(profile, path);
            let why = match vol {
                Some(v) if !mounts.path_is_on(path, &v.fs_uuid) => {
                    format!("its drive (filesystem UUID {}) isn't connected", v.fs_uuid)
                }
                Some(_) => return None,
                None => drive_absent(path, mounts)?,
            };
            Some(MissingSource {
                path: path.clone(),
                why,
                when_missing: vol.map(|v| v.when_missing).unwrap_or_default(),
                wait_for: vol
                    .and_then(|v| v.wait_for)
                    .map_or(DEFAULT_WAIT, |d| d.as_duration()),
            })
        })
        .collect()
}

/// Why `path`, under a removable-drive root, has no drive there; `None` if it looks fine
/// (or isn't under such a root).
fn drive_absent(path: &Path, mounts: &Mounts) -> Option<String> {
    let root = REMOVABLE_ROOTS
        .iter()
        .map(Path::new)
        .find(|r| path.starts_with(r))?;
    if let Mounts::Fake(_) = mounts {
        return (!mounts.on_extra_drive(path)).then(|| "no drive is mounted there".into());
    }
    let Ok(canon) = std::fs::canonicalize(path) else {
        return Some("it doesn't exist: the drive it's on isn't mounted".into());
    };
    match sys::mount_of(&canon) {
        Some(m) if !m.mount_point.starts_with(root) || m.mount_point == root => Some(
            "it's an empty folder on the system disk: the drive that belongs there isn't mounted"
                .into(),
        ),
        _ => None,
    }
}

/// The volume to record for a source on a drive of its own, if it is on one now.
pub fn volume_of(path: &Path, mounts: &Mounts) -> Option<SourceVolume> {
    if !mounts.on_extra_drive(path) {
        return None;
    }
    Some(SourceVolume {
        path: path.to_owned(),
        fs_uuid: mounts.fs_uuid_of(path)?,
        when_missing: WhenMissing::default(),
        wait_for: None,
    })
}

/// Record the drives of sources that are on one and have none recorded, and forget those
/// of sources no longer in the profile. Returns whether anything changed.
pub fn record_volumes(profile: &mut Profile) -> bool {
    record_volumes_in(profile, &Mounts::current())
}

/// [`record_volumes`] with these mounts.
pub fn record_volumes_in(profile: &mut Profile, mounts: &Mounts) -> bool {
    let before = profile.sources.volumes.clone();
    let paths = profile.sources.paths.clone();
    profile.sources.volumes.retain(|v| paths.contains(&v.path));
    for p in &paths {
        if volume_for(profile, p).is_none()
            && let Some(v) = volume_of(p, mounts)
        {
            profile.sources.volumes.push(v);
        }
    }
    profile.sources.volumes != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Target, WhenMissing};

    fn profile(sources: Vec<PathBuf>) -> Profile {
        Profile::new(
            "t",
            Target::Local {
                path: "/backup".into(),
                fs_uuid: None,
            },
            sources,
        )
    }

    fn fake(list: &[(&str, &Path)]) -> Mounts {
        Mounts::Fake(
            list.iter()
                .map(|(u, p)| (u.to_string(), p.to_path_buf()))
                .collect(),
        )
    }

    #[test]
    fn recorded_drives() {
        let photos = PathBuf::from("/mnt/photos");
        let home = PathBuf::from("/home/me");
        let plugged = fake(&[("1234-ABCD", &photos)]);
        let mut p = profile(vec![home, photos.clone()]);
        assert!(record_volumes_in(&mut p, &plugged));
        assert_eq!(p.sources.volumes.len(), 1, "only the source on a drive");
        assert_eq!(p.sources.volumes[0].fs_uuid, "1234-ABCD");
        assert!(!record_volumes_in(&mut p, &plugged), "nothing new");
        assert!(missing_sources_in(&p, &plugged).is_empty());

        // Unplugged: missing, with the default (skip).
        let m = missing_sources_in(&p, &fake(&[]));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, photos);
        assert_eq!(m[0].when_missing, WhenMissing::Skip);
        assert!(m[0].why.contains("1234-ABCD"), "{}", m[0].why);

        // Another drive at the same place is not the photos drive.
        assert_eq!(
            missing_sources_in(&p, &fake(&[("9999-0000", &photos)])).len(),
            1
        );

        // Wait, with its limit; a removed source forgets its drive.
        p.sources.volumes[0].when_missing = WhenMissing::Wait;
        p.sources.volumes[0].wait_for = Some("10m".parse().unwrap());
        let m = missing_sources_in(&p, &fake(&[]));
        assert_eq!(
            (m[0].when_missing, m[0].wait_for),
            (WhenMissing::Wait, Duration::from_secs(600))
        );
        p.sources.paths.retain(|x| x != &photos);
        assert!(record_volumes_in(&mut p, &plugged));
        assert!(p.sources.volumes.is_empty());
    }

    #[test]
    fn unrecorded_removable_paths() {
        let p = profile(vec![
            "/mnt/photos/2024".into(),
            "/run/media/me/Stick".into(),
            "/home/me".into(),
        ]);
        let m = missing_sources_in(&p, &fake(&[("AAAA", Path::new("/mnt/photos"))]));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].path, PathBuf::from("/run/media/me/Stick"));
    }

    #[test]
    fn real_system_paths() {
        // On the real system: a missing folder under /run/media is a missing drive; the
        // home folder (not under a drive root) never is.
        let p = profile(vec![
            "/run/media/nobody-here/NoSuchDrive".into(),
            sys::home_dir().unwrap_or_else(|| "/".into()),
        ]);
        let m = missing_sources_in(&p, &Mounts::Real);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].why.contains("isn't mounted"), "{}", m[0].why);
    }
}
