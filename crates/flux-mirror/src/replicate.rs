//! Replicas of plain-folder repositories (Mirror, and rsync's, which has the same layout):
//! make another repository hold exactly the same snapshots, copying only new snapshots, and
//! those as hard links wherever the source repository has them. Snapshots removed from the
//! source (pruned, deleted) are removed from the replica.
//!
//! ```text
//! <root>/flux-repo.toml          copied, with the replica's profile id
//! <root>/latest -> snapshots/<id>
//! <root>/snapshots/<id>/…        copied with flux_fs::replicate::copy_snapshot
//! <root>/<other files>           (rsync's recovery guide) copied when they differ
//! ```

use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use flux_core::progress::{Event, Progress, Reporter};
use flux_fs::fsops::{XattrPolicy, remove_force};
use flux_fs::replicate::{CopyStats, copy_snapshot};

use crate::MirrorError;
use crate::repo::{PARTIAL, REPO_MARKER, RepoInfo};

/// Where an unfinished copy of a snapshot is built (renamed into place when done).
const REPLICA_PARTIAL: &str = ".replica-partial";
const REPLICA_LOCK: &str = ".replica-lock";

/// What a replication did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaOutcome {
    /// Snapshots copied, oldest first.
    pub copied: Vec<String>,
    /// Snapshots removed from the replica (gone from the source).
    pub removed: Vec<String>,
    pub stats: CopyStats,
}

fn snapshot_names(root: &Path) -> Result<Vec<String>, MirrorError> {
    let dir = root.join("snapshots");
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(MirrorError::io(&dir, e)),
    };
    let mut names = Vec::new();
    for d in rd {
        let d = d.map_err(|e| MirrorError::io(&dir, e))?;
        let name = d.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && name != PARTIAL && d.path().is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Make the repository at `replica` hold the same snapshots as the one at `source`.
/// `replica_profile` goes into the replica's marker (the id it's opened under).
pub fn replicate(
    source: &Path,
    replica: &Path,
    replica_profile: &str,
    xattrs: XattrPolicy,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<ReplicaOutcome, MirrorError> {
    let marker = source.join(REPO_MARKER);
    let text = fs::read_to_string(&marker).map_err(|e| MirrorError::io(&marker, e))?;
    let mut info: RepoInfo = toml::from_str(&text)
        .map_err(|e| MirrorError::io(&marker, std::io::Error::other(e.to_string())))?;
    info.profile = replica_profile.to_owned();
    fs::create_dir_all(replica.join("snapshots")).map_err(|e| MirrorError::io(replica, e))?;
    // One replication at a time into a replica.
    let lock_path = replica.join(REPLICA_LOCK);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|e| MirrorError::io(&lock_path, e))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| MirrorError::Locked(replica.to_owned()))?;
    write_if_changed(
        &replica.join(REPO_MARKER),
        toml::to_string(&info)
            .map_err(|e| MirrorError::io(&marker, std::io::Error::other(e.to_string())))?
            .as_bytes(),
    )?;

    let have = snapshot_names(source)?;
    let mut there = snapshot_names(replica)?;
    let mut out = ReplicaOutcome::default();
    let partial = replica.join("snapshots").join(REPLICA_PARTIAL);
    let missing: Vec<String> = have
        .iter()
        .filter(|n| !there.contains(n))
        .cloned()
        .collect();
    for name in &missing {
        if progress.is_cancelled() {
            return Err(MirrorError::Cancelled);
        }
        // The newest snapshot already in the replica before this one, if the source has
        // it too: unchanged files are linked to it.
        let prev = there
            .iter()
            .filter(|p| p.as_str() < name.as_str() && have.contains(p))
            .max()
            .cloned();
        if partial.exists() {
            remove_force(&partial).map_err(|e| MirrorError::io(&partial, e))?;
        }
        let src = source.join("snapshots").join(name);
        let stats = copy_snapshot(
            &src,
            prev.as_ref()
                .map(|p| source.join("snapshots").join(p))
                .as_deref(),
            &partial,
            prev.as_ref()
                .map(|p| replica.join("snapshots").join(p))
                .as_deref(),
            xattrs,
            progress,
        )
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::Interrupted {
                MirrorError::Cancelled
            } else {
                MirrorError::io(&src, e)
            }
        })?;
        let dst = replica.join("snapshots").join(name);
        fs::rename(&partial, &dst).map_err(|e| MirrorError::io(&dst, e))?;
        report(Event::Info {
            message: format!(
                "copied snapshot {name} ({} files copied, {} linked)",
                stats.copied_files, stats.linked_files
            ),
        });
        out.stats.copied_files += stats.copied_files;
        out.stats.copied_bytes += stats.copied_bytes;
        out.stats.linked_files += stats.linked_files;
        out.stats.others += stats.others;
        out.copied.push(name.clone());
        there.push(name.clone());
        there.sort();
    }
    for name in there.iter().filter(|n| !have.contains(n)) {
        let dir = replica.join("snapshots").join(name);
        remove_force(&dir).map_err(|e| MirrorError::io(&dir, e))?;
        out.removed.push(name.clone());
    }
    there.retain(|n| have.contains(n));
    if let Some(newest) = there.last() {
        let link = replica.join("latest");
        let tmp = replica.join(".latest.tmp");
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(PathBuf::from("snapshots").join(newest), &tmp)
            .map_err(|e| MirrorError::io(&tmp, e))?;
        fs::rename(&tmp, &link).map_err(|e| MirrorError::io(&link, e))?;
    }
    // Other files at the top (rsync's recovery guide and script).
    let rd = fs::read_dir(source).map_err(|e| MirrorError::io(source, e))?;
    for d in rd.flatten() {
        let name = d.file_name();
        let n = name.to_string_lossy();
        if n.starts_with('.') || n == REPO_MARKER || !d.path().is_file() {
            continue;
        }
        let bytes = fs::read(d.path()).map_err(|e| MirrorError::io(&d.path(), e))?;
        write_if_changed(&replica.join(&name), &bytes)?;
        let mode = d
            .metadata()
            .map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()));
        if let Ok(mode) = mode {
            let _ = fs::set_permissions(
                replica.join(&name),
                std::os::unix::fs::PermissionsExt::from_mode(mode),
            );
        }
    }
    drop(lock);
    Ok(out)
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), MirrorError> {
    if fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    flux_core::store::write_atomic(path, bytes, 0o644)
        .map_err(|e| MirrorError::io(path, std::io::Error::other(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn snap(root: &Path, name: &str, files: &[(&str, &str)], link_from: Option<&str>) {
        let dir = root.join("snapshots").join(name).join("data");
        fs::create_dir_all(&dir).unwrap();
        for (f, text) in files {
            match (link_from, *text) {
                (Some(prev), "=") => fs::hard_link(
                    root.join("snapshots").join(prev).join("data").join(f),
                    dir.join(f),
                )
                .unwrap(),
                _ => fs::write(dir.join(f), text).unwrap(),
            }
        }
        fs::write(root.join("snapshots").join(name).join("meta.json"), "{}").unwrap();
    }

    #[test]
    fn copies_new_removes_gone_and_links_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let (src, rep) = (t.path().join("src"), t.path().join("rep"));
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join(REPO_MARKER),
            "format = 1\nengine = \"mirror\"\nprofile = \"home\"\nhost = \"h\"\nuser = \"u\"\ncreated = \"2026-11-09T00:00:00Z\"\n",
        )
        .unwrap();
        fs::write(src.join("FLUX-RECOVERY.txt"), "how to restore").unwrap();
        snap(&src, "2026-11-01", &[("a", "same"), ("b", "one")], None);
        let p = Progress::new();
        let none = XattrPolicy {
            acl: false,
            other: false,
        };
        let o = replicate(
            &src,
            &rep,
            "home+usb",
            none,
            &p,
            &flux_core::progress::silent,
        )
        .unwrap();
        assert_eq!(o.copied, ["2026-11-01"]);
        let info: RepoInfo =
            toml::from_str(&fs::read_to_string(rep.join(REPO_MARKER)).unwrap()).unwrap();
        assert_eq!(info.profile, "home+usb");
        assert_eq!(
            fs::read_to_string(rep.join("FLUX-RECOVERY.txt")).unwrap(),
            "how to restore"
        );

        // A second snapshot with `a` unchanged; nothing to do the time after.
        snap(
            &src,
            "2026-11-02",
            &[("a", "="), ("b", "two")],
            Some("2026-11-01"),
        );
        let o = replicate(
            &src,
            &rep,
            "home+usb",
            none,
            &p,
            &flux_core::progress::silent,
        )
        .unwrap();
        assert_eq!(o.copied, ["2026-11-02"]);
        assert_eq!(
            (o.stats.copied_files, o.stats.linked_files),
            (2, 1),
            "meta.json, b; a linked"
        );
        let ino = |s: &str, f: &str| {
            fs::metadata(rep.join("snapshots").join(s).join("data").join(f))
                .unwrap()
                .ino()
        };
        assert_eq!(ino("2026-11-01", "a"), ino("2026-11-02", "a"));
        assert_eq!(
            fs::read_link(rep.join("latest")).unwrap(),
            Path::new("snapshots/2026-11-02")
        );
        let again = replicate(
            &src,
            &rep,
            "home+usb",
            none,
            &p,
            &flux_core::progress::silent,
        )
        .unwrap();
        assert!(again.copied.is_empty() && again.removed.is_empty());

        // Pruned at the source: removed from the replica; the rest still reads.
        remove_force(&src.join("snapshots/2026-11-01")).unwrap();
        let o = replicate(
            &src,
            &rep,
            "home+usb",
            none,
            &p,
            &flux_core::progress::silent,
        )
        .unwrap();
        assert_eq!(o.removed, ["2026-11-01"]);
        assert_eq!(
            fs::read_to_string(rep.join("snapshots/2026-11-02/data/a")).unwrap(),
            "same"
        );
    }
}
