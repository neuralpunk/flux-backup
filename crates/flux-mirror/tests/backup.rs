//! End-to-end tests of the Mirror engine on real temporary directories.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use flux_core::profile::{Profile, Target};
use flux_core::progress::{Progress, silent};
use flux_core::{ByteSize, ProfileId};
use flux_fs::restore::RestoreRequest;
use flux_mirror::fsops::XattrPolicy;
use flux_mirror::manifest::{Kind, ManifestReader};
use flux_mirror::repo::SnapshotStatus;
use flux_mirror::restore;
use flux_mirror::{BackupOutcome, BackupRequest, MirrorError, backup, open_repo};

struct Env {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    target: PathBuf,
    id: ProfileId,
    profile: Profile,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let target = tmp.path().join("target");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&target).unwrap();
    let profile = Profile::new(
        "test",
        Target::Local {
            path: target.clone(),
            fs_uuid: None,
        },
        vec![src.clone()],
    );
    Env {
        _tmp: tmp,
        src,
        target,
        id: ProfileId::new("test").unwrap(),
        profile,
    }
}

impl Env {
    fn run(&self) -> Result<BackupOutcome, MirrorError> {
        self.run_named(None)
    }
    fn run_named(&self, name: Option<&str>) -> Result<BackupOutcome, MirrorError> {
        backup(
            &BackupRequest {
                id: &self.id,
                profile: &self.profile,
                name: name.map(str::to_owned),
                dry_run: false,
                prune: false,
            },
            &Progress::new(),
            &silent,
        )
    }
    fn created(&self) -> flux_mirror::repo::SnapshotMeta {
        match self.run().unwrap() {
            BackupOutcome::Created { snapshot, .. } => *snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }
    /// Path of a source file inside a snapshot's data tree.
    fn in_snapshot(&self, id: &str, src_path: &Path) -> PathBuf {
        let repo = open_repo(&self.id, &self.profile).unwrap();
        repo.data_dir(id).join(src_path.strip_prefix("/").unwrap())
    }
}

fn write(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// Assert two trees have the same names, kinds, contents, modes, mtimes and link targets.
fn assert_same_tree(a: &Path, b: &Path) {
    let ma = fs::symlink_metadata(a).unwrap();
    let mb = fs::symlink_metadata(b).unwrap();
    assert_eq!(ma.file_type(), mb.file_type(), "{}", b.display());
    if !ma.file_type().is_symlink() {
        assert_eq!(
            ma.mode() & 0o7777,
            mb.mode() & 0o7777,
            "mode of {}",
            b.display()
        );
    }
    assert_eq!(
        (ma.mtime(), ma.mtime_nsec()),
        (mb.mtime(), mb.mtime_nsec()),
        "mtime of {}",
        b.display()
    );
    if ma.is_file() {
        assert_eq!(
            fs::read(a).unwrap(),
            fs::read(b).unwrap(),
            "content of {}",
            b.display()
        );
    } else if ma.file_type().is_symlink() {
        assert_eq!(fs::read_link(a).unwrap(), fs::read_link(b).unwrap());
    } else if ma.is_dir() {
        let names = |d: &Path| {
            let mut v: Vec<_> = fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            v.sort();
            v
        };
        let (na, nb) = (names(a), names(b));
        assert_eq!(na, nb, "entries of {}", b.display());
        for n in na {
            assert_same_tree(&a.join(&n), &b.join(&n));
        }
    }
}

fn sample_tree(src: &Path) {
    write(&src.join("docs/readme.txt"), "hello");
    write(&src.join("docs/deep/nested/file.bin"), &"x".repeat(100_000));
    write(&src.join("big.iso"), &"y".repeat(5000));
    fs::create_dir_all(src.join("empty")).unwrap();
    symlink("docs/readme.txt", src.join("link")).unwrap();
    symlink("/nonexistent/target", src.join("dangling")).unwrap();
    write(&src.join("script.sh"), "#!/bin/sh\n");
    fs::set_permissions(src.join("script.sh"), fs::Permissions::from_mode(0o750)).unwrap();
    write(&src.join("ro/locked.txt"), "can't touch this");
    fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
}

#[test]
fn full_then_incremental() {
    let e = env();
    sample_tree(&e.src);

    let s1 = e.created();
    assert_eq!(s1.status, SnapshotStatus::Complete, "{:?}", s1.errors);
    assert_eq!(s1.stats.copied_files, s1.stats.files);
    assert_same_tree(&e.src, &e.in_snapshot(&s1.id, &e.src));

    // Nothing changed → no new snapshot.
    assert!(matches!(e.run().unwrap(), BackupOutcome::Unchanged { .. }));

    // Change one file, add one, delete one.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(e.src.join("docs/readme.txt"), "hello, world").unwrap();
    write(&e.src.join("docs/new.txt"), "new");
    fs::remove_file(e.src.join("big.iso")).unwrap();

    let s2 = e.created();
    assert_eq!(
        (s2.stats.new, s2.stats.changed, s2.stats.deleted),
        (1, 1, 1),
        "{:?}",
        s2.stats
    );
    assert_eq!(s2.stats.copied_files, 2);
    assert_same_tree(&e.src, &e.in_snapshot(&s2.id, &e.src));

    // Unchanged files share an inode with the previous snapshot.
    let f = e.src.join("docs/deep/nested/file.bin");
    let i1 = fs::metadata(e.in_snapshot(&s1.id, &f)).unwrap().ino();
    let i2 = fs::metadata(e.in_snapshot(&s2.id, &f)).unwrap().ino();
    assert_eq!(i1, i2);
    // The old snapshot is untouched.
    assert_eq!(
        fs::read_to_string(e.in_snapshot(&s1.id, &e.src.join("docs/readme.txt"))).unwrap(),
        "hello"
    );
    assert!(e.in_snapshot(&s1.id, &e.src.join("big.iso")).exists());

    // Manifest and listing agree.
    let repo = open_repo(&e.id, &e.profile).unwrap();
    let entries = ManifestReader::read_all(&repo.manifest_path(&s2.id)).unwrap();
    assert!(
        entries.windows(2).all(|w| w[0].path < w[1].path),
        "manifest sorted"
    );
    assert_eq!(entries.len() as u64, s2.stats.entries);
    let snaps = repo.snapshots().unwrap();
    assert_eq!(
        snaps.iter().map(|s| &s.id).collect::<Vec<_>>(),
        [&s1.id, &s2.id]
    );
    assert_eq!(repo.find("latest").unwrap().id, s2.id);
    assert_eq!(
        fs::read_link(repo.root().join("latest")).unwrap(),
        Path::new("snapshots").join(&s2.id)
    );

    // A named run creates a snapshot even without changes.
    let BackupOutcome::Created { snapshot, .. } = e.run_named(Some("before-upgrade")).unwrap()
    else {
        panic!()
    };
    assert_eq!(repo.find("before-upgrade").unwrap().id, snapshot.id);

    // Clean up read-only dirs so TempDir can delete them.
    fs::set_permissions(e.src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
    flux_mirror::fsops::make_tree_writable(&e.target).unwrap();
}

#[test]
fn excludes() {
    let mut e = env();
    write(&e.src.join("keep.txt"), "k");
    write(&e.src.join("skip.tmp"), "s");
    write(&e.src.join("node_modules/pkg/index.js"), "x");
    write(&e.src.join("huge.bin"), &"z".repeat(2000));
    write(
        &e.src.join("cache/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    );
    write(&e.src.join("cache/blob"), "c");
    write(&e.src.join("private/.nobackup"), "");
    write(&e.src.join("private/secret"), "s");
    e.profile.sources.exclude = vec!["*.tmp".into(), "node_modules/".into()];
    e.profile.sources.exclude_larger_than = Some(ByteSize(1000));
    e.profile.sources.exclude_if_present = vec![".nobackup".into()];

    let s = e.created();
    let root = e.in_snapshot(&s.id, &e.src);
    assert!(root.join("keep.txt").exists());
    for gone in [
        "skip.tmp",
        "node_modules",
        "huge.bin",
        "cache/blob",
        "private/secret",
    ] {
        assert!(!root.join(gone).exists(), "{gone} should be excluded");
    }
}

#[test]
fn target_inside_source_is_skipped() {
    let mut e = env();
    write(&e.src.join("a.txt"), "a");
    let inner = e.src.join("backups");
    fs::create_dir_all(&inner).unwrap();
    e.profile.target = Target::Local {
        path: inner.clone(),
        fs_uuid: None,
    };
    let s1 = e.created();
    write(&e.src.join("b.txt"), "b");
    let s2 = e.created();
    let snap_root = e.in_snapshot(&s2.id, &e.src);
    assert!(snap_root.join("b.txt").exists());
    assert!(
        !snap_root.join("backups/flux-backup").exists(),
        "must not back up itself"
    );
    assert_ne!(s1.id, s2.id);
}

#[test]
fn special_files_xattrs_and_hardlinks() {
    let e = env();
    write(&e.src.join("a"), "shared");
    fs::hard_link(e.src.join("a"), e.src.join("b")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        e.src.join("fifo"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o644),
        0,
    )
    .unwrap();
    let xattr_ok = rustix::fs::lsetxattr(
        e.src.join("a"),
        "user.flux.test",
        b"v1",
        rustix::fs::XattrFlags::empty(),
    )
    .is_ok();

    let s = e.created();
    let root = e.in_snapshot(&s.id, &e.src);
    let (ma, mb) = (
        fs::metadata(root.join("a")).unwrap(),
        fs::metadata(root.join("b")).unwrap(),
    );
    assert_eq!(ma.ino(), mb.ino(), "source hard links are preserved");
    assert!(
        fs::symlink_metadata(root.join("fifo"))
            .unwrap()
            .file_type()
            .is_fifo_like()
    );
    if xattr_ok {
        let mut buf = [0u8; 16];
        let n = rustix::fs::lgetxattr(root.join("a"), "user.flux.test", &mut buf[..]).unwrap();
        assert_eq!(&buf[..n], b"v1");
        let entries =
            ManifestReader::read_all(&open_repo(&e.id, &e.profile).unwrap().manifest_path(&s.id))
                .unwrap();
        let a = entries.iter().find(|x| x.path.ends_with(b"/a")).unwrap();
        assert_eq!(a.xattrs.len(), 1);
    }
}

trait FifoExt {
    fn is_fifo_like(&self) -> bool;
}
impl FifoExt for fs::FileType {
    fn is_fifo_like(&self) -> bool {
        std::os::unix::fs::FileTypeExt::is_fifo(self)
    }
}

#[test]
fn resumes_interrupted_backup() {
    let e = env();
    sample_tree(&e.src);
    // Fake an interrupted run: a .partial with a stale file and a truncated copy.
    let s0 = e.created();
    write(&e.src.join("docs/new.txt"), "fresh");
    let repo = open_repo(&e.id, &e.profile).unwrap();
    let pdata = repo
        .partial_dir()
        .join("data")
        .join(e.src.strip_prefix("/").unwrap());
    write(&pdata.join("stale-leftover"), "junk");
    write(&pdata.join("docs/new.txt"), "fr"); // truncated
    let s1 = e.created();
    assert_ne!(s0.id, s1.id);
    assert!(!repo.partial_dir().exists());
    assert_same_tree(&e.src, &e.in_snapshot(&s1.id, &e.src));
    fs::set_permissions(e.src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
    flux_mirror::fsops::make_tree_writable(&e.target).unwrap();
}

#[test]
fn dry_run_and_lock() {
    let e = env();
    write(&e.src.join("f"), "12345");
    let out = backup(
        &BackupRequest {
            id: &e.id,
            profile: &e.profile,
            name: None,
            dry_run: true,
            prune: false,
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    let BackupOutcome::DryRun { stats, .. } = out else {
        panic!()
    };
    assert_eq!((stats.copy_files, stats.copy_bytes), (1, 5));
    assert!(
        !e.target.join("flux-backup").exists(),
        "dry run writes nothing"
    );

    e.created();
    let repo = open_repo(&e.id, &e.profile).unwrap();
    let _held = repo.lock().unwrap();
    assert!(matches!(e.run(), Err(MirrorError::Locked(_))));
}

#[test]
fn missing_sources() {
    let mut e = env();
    e.profile.sources.paths = vec![e.src.join("nope")];
    assert!(matches!(e.run(), Err(MirrorError::NoSources)));
}

#[test]
fn unreadable_file_is_reported_not_fatal() {
    if flux_core::sys::is_root() {
        return; // root can read everything
    }
    let e = env();
    write(&e.src.join("ok"), "fine");
    write(&e.src.join("secret"), "no");
    fs::set_permissions(e.src.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();
    let s = e.created();
    assert_eq!(s.status, SnapshotStatus::CompleteWithErrors);
    assert_eq!(s.errors_total, 1);
    assert!(s.errors[0].path.ends_with("secret"));
    assert!(e.in_snapshot(&s.id, &e.src.join("ok")).exists());
    // Left out of the manifest, so the next run retries it.
    let entries =
        ManifestReader::read_all(&open_repo(&e.id, &e.profile).unwrap().manifest_path(&s.id))
            .unwrap();
    assert!(!entries.iter().any(|x| x.path.ends_with(b"secret")));
    fs::set_permissions(e.src.join("secret"), fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn restore_elsewhere_and_in_place() {
    let e = env();
    sample_tree(&e.src);
    let s = e.created();
    let repo = open_repo(&e.id, &e.profile).unwrap();
    let req = |paths: Vec<PathBuf>, to: Option<PathBuf>, keep: bool| RestoreRequest {
        snapshot: &s,
        paths,
        to,
        keep_existing: keep,
        dry_run: false,
        xattrs: XattrPolicy {
            acl: true,
            other: true,
        },
        links: flux_fs::safe::LinkPolicy::for_process(),
    };

    // Whole source to another directory.
    let out_dir = e.target.join("restored");
    let r = restore(
        &repo,
        &req(vec![e.src.clone()], Some(out_dir.clone()), true),
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_same_tree(&e.src, &out_dir.join("src"));

    // In place: a deleted file comes back, a modified one is moved aside first.
    fs::remove_file(e.src.join("docs/readme.txt")).unwrap();
    fs::write(e.src.join("script.sh"), "changed").unwrap();
    let r = restore(
        &repo,
        &req(
            vec![e.src.join("docs"), e.src.join("script.sh")],
            None,
            true,
        ),
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(
        fs::read_to_string(e.src.join("docs/readme.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        fs::read_to_string(e.src.join("script.sh")).unwrap(),
        "#!/bin/sh\n"
    );
    assert_eq!(r.moved_aside.len(), 1);
    assert_eq!(fs::read_to_string(&r.moved_aside[0]).unwrap(), "changed");
    assert!(r.skipped_identical > 0);

    // Unknown path.
    assert!(matches!(
        restore(
            &repo,
            &req(vec![e.src.join("nope")], None, true),
            &Progress::new(),
            &silent
        ),
        Err(MirrorError::NotInSnapshot(..))
    ));

    let kinds: Vec<_> = ManifestReader::read_all(&repo.manifest_path(&s.id))
        .unwrap()
        .iter()
        .map(|x| x.kind)
        .collect();
    assert!(
        kinds.contains(&Kind::Symlink) && kinds.contains(&Kind::Dir) && kinds.contains(&Kind::File)
    );
    fs::set_permissions(e.src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
    flux_mirror::fsops::make_tree_writable(&e.target).unwrap();
}

/// Restores as root must not be redirected by symlinks that someone planted in the
/// destination. The test isn't root, so it plays root's policy by trusting only another
/// user's links: every link it creates itself counts as planted.
#[test]
fn restore_is_not_redirected_by_planted_symlinks() {
    use flux_fs::safe::LinkPolicy;
    let e = env();
    write(&e.src.join("docs/a.txt"), "from backup");
    write(&e.src.join("b.txt"), "from backup");
    let s = e.created();
    let repo = open_repo(&e.id, &e.profile).unwrap();
    let base = e._tmp.path().canonicalize().unwrap();
    let outside = base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("b.txt"), "precious").unwrap();
    let strict = LinkPolicy::TrustOwners(vec![flux_core::sys::euid().wrapping_add(4242)]);
    let req = |paths: Vec<PathBuf>, to: Option<PathBuf>, keep: bool| RestoreRequest {
        snapshot: &s,
        paths,
        to,
        keep_existing: keep,
        dry_run: false,
        xattrs: XattrPolicy {
            acl: false,
            other: false,
        },
        links: strict.clone(),
    };

    // 1. A folder being restored is a link to somewhere else: the link is replaced by a
    //    real folder, nothing is written through it.
    let dest = base.join("dest1");
    fs::create_dir_all(dest.join("src")).unwrap();
    symlink(&outside, dest.join("src/docs")).unwrap();
    let r = restore(
        &repo,
        &req(vec![e.src.clone()], Some(dest.clone()), false),
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(!outside.join("a.txt").exists(), "wrote through the link");
    assert!(
        !fs::symlink_metadata(dest.join("src/docs"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(dest.join("src/docs/a.txt")).unwrap(),
        "from backup"
    );

    // 2. A file to restore is a link to a precious file: the link is replaced, the target
    //    is untouched.
    let dest = base.join("dest2");
    fs::create_dir_all(dest.join("src")).unwrap();
    symlink(outside.join("b.txt"), dest.join("src/b.txt")).unwrap();
    let r = restore(
        &repo,
        &req(vec![e.src.join("b.txt")], Some(dest.join("src")), false),
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(
        fs::read_to_string(outside.join("b.txt")).unwrap(),
        "precious"
    );
    let restored = dest.join("src/b.txt");
    assert!(!fs::symlink_metadata(&restored).unwrap().is_symlink());
    assert_eq!(fs::read_to_string(&restored).unwrap(), "from backup");

    // 3. The destination folder itself is reached through an untrusted link: refused
    //    before anything happens.
    symlink(&outside, base.join("dest3")).unwrap();
    assert!(
        restore(
            &repo,
            &req(vec![e.src.join("b.txt")], Some(base.join("dest3/x")), false),
            &Progress::new(),
            &silent,
        )
        .is_err()
    );
    assert!(!outside.join("x").exists());
}
