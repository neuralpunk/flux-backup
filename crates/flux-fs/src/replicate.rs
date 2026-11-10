//! Copying a snapshot folder to another drive, keeping it as small there as it is where it
//! came from: what [`crate::replicate::copy_snapshot`] does for replicas of Mirror and rsync
//! backups.
//!
//! In a repository of plain-folder snapshots, a file unchanged since the previous snapshot
//! is the same inode in both (a hard link). So the copy knows exactly which files are
//! unchanged without comparing anything: those whose inode is the previous snapshot's are
//! hard-linked to the replica's copy of the previous snapshot; the others are copied.
//! Hard links inside one snapshot (the source's own) are kept too.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;

use flux_core::progress::Progress;

use crate::entry::{Entry, Kind};
use crate::fsops::{ApplyPolicy, Copier, XattrPolicy, apply_metadata, make_special, read_xattrs};

/// What a copy did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// Files copied, and their bytes.
    pub copied_files: u64,
    pub copied_bytes: u64,
    /// Files hard-linked (to the previous snapshot's copy, or inside the snapshot).
    pub linked_files: u64,
    /// Folders, links and special files made.
    pub others: u64,
}

/// The metadata of `path` as an [`Entry`] (for [`apply_metadata`]).
fn entry_of(path: &Path, m: &fs::Metadata, xattrs: XattrPolicy) -> io::Result<Entry> {
    let kind = Kind::from_file_type(m.file_type());
    Ok(Entry {
        path: Vec::new(),
        kind,
        mode: m.mode() & 0o7777,
        uid: m.uid(),
        gid: m.gid(),
        size: if kind == Kind::File { m.len() } else { 0 },
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec() as u32,
        dev: m.dev(),
        ino: m.ino(),
        nlink: m.nlink(),
        rdev: m.rdev(),
        link: None,
        hash: None,
        xattrs: if kind == Kind::Symlink {
            Vec::new()
        } else {
            read_xattrs(path, xattrs).unwrap_or_default()
        },
    })
}

/// Copy snapshot folder `src` to `dst` (which must not exist). `src_prev`: the snapshot
/// before `src` where it came from; `dst_prev`: that snapshot's copy here. Files of `src`
/// that are the same inode as in `src_prev` become hard links to `dst_prev`.
pub fn copy_snapshot(
    src: &Path,
    src_prev: Option<&Path>,
    dst: &Path,
    dst_prev: Option<&Path>,
    xattrs: XattrPolicy,
    progress: &Progress,
) -> io::Result<CopyStats> {
    let mut ctx = Ctx {
        src_prev,
        dst_prev,
        xattrs,
        policy: ApplyPolicy::new(xattrs),
        copier: Copier::default(),
        progress,
        seen: HashMap::new(),
        stats: CopyStats::default(),
    };
    ctx.copy(src, dst, Path::new(""))?;
    Ok(ctx.stats)
}

struct Ctx<'a> {
    src_prev: Option<&'a Path>,
    dst_prev: Option<&'a Path>,
    xattrs: XattrPolicy,
    policy: ApplyPolicy,
    copier: Copier,
    progress: &'a Progress,
    /// (dev, ino) of files copied so far → their copy (hard links inside the snapshot).
    seen: HashMap<(u64, u64), std::path::PathBuf>,
    stats: CopyStats,
}

impl Ctx<'_> {
    fn copy(&mut self, src: &Path, dst: &Path, rel: &Path) -> io::Result<()> {
        if self.progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let m = fs::symlink_metadata(src)?;
        let e = entry_of(src, &m, self.xattrs)?;
        match e.kind {
            Kind::Dir => {
                fs::create_dir(dst)?;
                let mut names: Vec<_> = fs::read_dir(src)?.collect::<Result<_, _>>()?;
                names.sort_by_key(|d| d.file_name());
                for d in names {
                    let name = d.file_name();
                    self.copy(&d.path(), &dst.join(&name), &rel.join(&name))?;
                }
                // Folders last, so writing into them didn't change their times.
                apply_metadata(dst, &e, &self.policy)?;
                self.stats.others += 1;
            }
            Kind::File => {
                let key = (m.dev(), m.ino());
                if let Some(first) = self.seen.get(&key) {
                    fs::hard_link(first, dst)?;
                    self.stats.linked_files += 1;
                    return Ok(());
                }
                let unchanged = match (self.src_prev, self.dst_prev) {
                    (Some(sp), Some(dp)) => fs::symlink_metadata(sp.join(rel))
                        .is_ok_and(|pm| (pm.dev(), pm.ino()) == key)
                        .then(|| dp.join(rel))
                        .filter(|d| d.is_file()),
                    _ => None,
                };
                match unchanged {
                    Some(prev_copy) => {
                        fs::hard_link(&prev_copy, dst)?;
                        self.stats.linked_files += 1;
                    }
                    None => {
                        let n = self.copier.copy(src, dst, self.progress)?;
                        apply_metadata(dst, &e, &self.policy)?;
                        self.stats.copied_files += 1;
                        self.stats.copied_bytes += n;
                    }
                }
                self.seen.insert(key, dst.to_owned());
            }
            Kind::Symlink => {
                symlink(fs::read_link(src)?, dst)?;
                apply_metadata(dst, &e, &self.policy)?;
                self.stats.others += 1;
            }
            Kind::Fifo | Kind::CharDevice | Kind::BlockDevice => {
                if e.kind == Kind::Fifo || self.policy.privileged {
                    make_special(dst, &e)?;
                    apply_metadata(dst, &e, &self.policy)?;
                    self.stats.others += 1;
                }
            }
            Kind::Socket => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn copies_change_and_links_the_rest() {
        let t = tempfile::tempdir().unwrap();
        let (main, rep) = (t.path().join("main"), t.path().join("replica"));
        // Snapshot 1 where it came from, and its copy here.
        write(&main.join("s1/a.txt"), "same");
        write(&main.join("s1/b.txt"), "old");
        fs::create_dir_all(&rep).unwrap();
        let p = Progress::new();
        let none = XattrPolicy {
            acl: false,
            other: false,
        };
        let s = copy_snapshot(&main.join("s1"), None, &rep.join("s1"), None, none, &p).unwrap();
        assert_eq!((s.copied_files, s.linked_files), (2, 0));

        // Snapshot 2: a.txt unchanged (a hard link where it came from), b.txt changed, a
        // new file with a second name (a hard link inside the snapshot), a symlink.
        fs::create_dir_all(main.join("s2")).unwrap();
        fs::hard_link(main.join("s1/a.txt"), main.join("s2/a.txt")).unwrap();
        write(&main.join("s2/b.txt"), "new!");
        write(&main.join("s2/c.txt"), "twin");
        fs::hard_link(main.join("s2/c.txt"), main.join("s2/c2.txt")).unwrap();
        symlink("a.txt", main.join("s2/link")).unwrap();
        let s = copy_snapshot(
            &main.join("s2"),
            Some(&main.join("s1")),
            &rep.join("s2"),
            Some(&rep.join("s1")),
            none,
            &p,
        )
        .unwrap();
        assert_eq!(s.copied_files, 2, "b.txt and c.txt");
        assert_eq!(
            s.linked_files, 2,
            "a.txt to the previous copy, c2.txt to c.txt"
        );
        let ino = |p: &Path| fs::metadata(p).unwrap().ino();
        assert_eq!(ino(&rep.join("s2/a.txt")), ino(&rep.join("s1/a.txt")));
        assert_ne!(ino(&rep.join("s2/b.txt")), ino(&rep.join("s1/b.txt")));
        assert_eq!(ino(&rep.join("s2/c.txt")), ino(&rep.join("s2/c2.txt")));
        assert_eq!(fs::read_to_string(rep.join("s2/b.txt")).unwrap(), "new!");
        assert_eq!(
            fs::read_link(rep.join("s2/link")).unwrap(),
            Path::new("a.txt")
        );
        assert_eq!(
            fs::metadata(rep.join("s2/b.txt")).unwrap().mtime(),
            fs::metadata(main.join("s2/b.txt")).unwrap().mtime(),
            "times kept"
        );
    }
}
