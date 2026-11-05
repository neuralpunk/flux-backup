//! End-to-end tests of the Vault engine on real temporary directories.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use flux_core::profile::{Profile, Retention, Target, VaultOptions};
use flux_core::progress::{Progress, silent};
use flux_fs::fsops::XattrPolicy;
use flux_fs::outcome::BackupOutcome;
use flux_fs::restore::{RestoreRequest, restore};
use flux_vault::backup::{BackupRequest, backup};
use flux_vault::crypto::KdfParams;
use flux_vault::{LocalBackend, Owner, VaultError, VaultRepo, VaultSource, check, gc};

const PW: &str = "correct horse battery staple";

struct Env {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    root: PathBuf,
    profile: Profile,
    owner: Owner,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let target = tmp.path().join("target");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&target).unwrap();
    let mut profile = Profile::new(
        "t",
        Target::Local {
            path: target.clone(),
            fs_uuid: None,
        },
        vec![src.clone()],
    );
    profile.engine = flux_core::Engine::Vault;
    profile.vault = Some(VaultOptions::default());
    let owner = Owner {
        profile: "t".into(),
        host: "h".into(),
        user: "u".into(),
    };
    Env {
        _tmp: tmp,
        src,
        root: flux_vault::local_root(&target),
        profile,
        owner,
    }
}

impl Env {
    fn init(&self, password: Option<&str>) -> VaultRepo {
        VaultRepo::init(
            Box::new(LocalBackend::new(&self.root)),
            password,
            KdfParams::TESTING,
        )
        .unwrap()
    }
    fn open(&self) -> VaultRepo {
        VaultRepo::open(Box::new(LocalBackend::new(&self.root)), Some(PW)).unwrap()
    }
    fn backup(&self, repo: &VaultRepo) -> BackupOutcome {
        backup(
            repo,
            &BackupRequest {
                owner: &self.owner,
                profile: &self.profile,
                name: None,
                dry_run: false,
                never: vec![self.root.clone()],
            },
            &Progress::new(),
            &silent,
        )
        .unwrap()
    }
    fn snap(&self, repo: &VaultRepo) -> flux_fs::snapshot::SnapshotMeta {
        match self.backup(repo) {
            BackupOutcome::Created { snapshot, .. } => *snapshot,
            o => panic!("expected a snapshot: {o:?}"),
        }
    }
    fn restore_to(
        &self,
        repo: &VaultRepo,
        snap: &flux_fs::snapshot::SnapshotMeta,
        to: &Path,
    ) -> flux_fs::restore::RestoreOutcome {
        let source = VaultSource::new(repo, &snap.id).unwrap();
        restore(
            &source,
            &RestoreRequest {
                snapshot: snap,
                paths: vec![self.src.clone()],
                to: Some(to.to_owned()),
                keep_existing: true,
                dry_run: false,
                xattrs: XattrPolicy {
                    acl: true,
                    other: true,
                },
                links: flux_fs::safe::LinkPolicy::for_process(),
            },
            &Progress::new(),
            &silent,
        )
        .unwrap()
    }
}

fn write(p: &Path, content: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// Pseudo-random bytes (incompressible, deterministic).
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

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
        assert!(
            fs::read(a).unwrap() == fs::read(b).unwrap(),
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
        assert_eq!(names(a), names(b), "entries of {}", b.display());
        for n in names(a) {
            assert_same_tree(&a.join(&n), &b.join(&n));
        }
    }
}

fn sample(src: &Path) {
    write(
        &src.join("docs/readme.txt"),
        b"MARKER-PLAINTEXT hello vault",
    );
    write(&src.join("docs/deep/big.bin"), &noise(3 << 20, 1));
    write(&src.join("copy-of-big.bin"), &noise(3 << 20, 1)); // duplicate content
    write(
        &src.join("text.log"),
        &b"compressible line\n".repeat(50_000),
    );
    write(&src.join("empty"), b"");
    fs::create_dir_all(src.join("emptydir")).unwrap();
    symlink("docs/readme.txt", src.join("link")).unwrap();
    write(&src.join("script.sh"), b"#!/bin/sh\n");
    fs::set_permissions(src.join("script.sh"), fs::Permissions::from_mode(0o750)).unwrap();
}

fn all_repo_bytes(root: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.extend(fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

#[test]
fn encrypted_round_trip_with_dedup_and_compression() {
    let e = env();
    sample(&e.src);
    let repo = e.init(Some(PW));
    let s1 = e.snap(&repo);
    assert_eq!(s1.stats.files, 6);
    // Duplicate 3 MiB file stored once; the log compresses well.
    let source_bytes = s1.stats.total_bytes;
    assert!(
        s1.stats.stored_bytes < source_bytes * 6 / 10,
        "stored {} of {}",
        s1.stats.stored_bytes,
        source_bytes
    );

    // Nothing readable on disk.
    let raw = all_repo_bytes(&e.root);
    assert!(!raw.windows(16).any(|w| w == b"MARKER-PLAINTEXT"));
    assert!(!raw.windows(17).any(|w| w == b"compressible line"));

    // A fresh process (new open) restores everything faithfully.
    drop(repo);
    let repo = e.open();
    let out = e._tmp.path().join("restored");
    let r = e.restore_to(&repo, &s1, &out);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_same_tree(&e.src, &out.join("src"));

    // No changes → no new snapshot.
    assert!(matches!(e.backup(&repo), BackupOutcome::Unchanged { .. }));

    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);
    assert!(report.packs_read > 0);
}

#[test]
fn small_edit_in_large_file_stores_little() {
    let e = env();
    let mut big = noise(24 << 20, 7);
    write(&e.src.join("big.bin"), &big);
    let repo = e.init(Some(PW));
    let s1 = e.snap(&repo);
    // Insert bytes in the middle: content-defined chunking resyncs after the edit.
    big.splice(12 << 20..12 << 20, b"inserted!".iter().copied());
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(&e.src.join("big.bin"), &big);
    let s2 = e.snap(&repo);
    assert!(s1.stats.stored_bytes > 20 << 20);
    assert!(
        s2.stats.stored_bytes < 10 << 20,
        "stored {} after a 9-byte insert",
        s2.stats.stored_bytes
    );
    let out = e._tmp.path().join("r2");
    e.restore_to(&repo, &s2, &out);
    assert!(fs::read(out.join("src/big.bin")).unwrap() == big);
}

#[test]
fn passwords_and_key_slots() {
    let e = env();
    write(&e.src.join("f"), b"x");
    let repo = e.init(Some(PW));
    e.snap(&repo);
    let b = || Box::new(LocalBackend::new(&e.root));
    assert!(matches!(
        VaultRepo::open(b(), Some("nope")),
        Err(VaultError::WrongPassword)
    ));
    assert!(matches!(
        VaultRepo::open(b(), None),
        Err(VaultError::PasswordRequired)
    ));

    let mut repo = e.open();
    repo.kdf = KdfParams::TESTING;
    let first = repo.slot.clone().unwrap();
    let second = repo.add_key("second password", "laptop").unwrap();
    let r2 = VaultRepo::open(b(), Some("second password")).unwrap();
    assert_eq!(r2.slot.as_deref(), Some(second.as_str()));
    assert_eq!(
        r2.snapshots(&e.owner).unwrap().len(),
        1,
        "same data under both keys"
    );
    r2.remove_key(&first).unwrap();
    assert!(matches!(
        VaultRepo::open(b(), Some(PW)),
        Err(VaultError::WrongPassword)
    ));
    assert!(matches!(r2.remove_key(&second), Err(VaultError::LastKey)));
}

#[test]
fn check_detects_damage() {
    let e = env();
    write(&e.src.join("a"), &noise(2 << 20, 3));
    let repo = e.init(Some(PW));
    e.snap(&repo);
    let packs: Vec<PathBuf> = fs::read_dir(e.root.join("data"))
        .unwrap()
        .flat_map(|d| {
            fs::read_dir(d.unwrap().path())
                .unwrap()
                .map(|f| f.unwrap().path())
        })
        .collect();
    // Flip one byte in the middle of the biggest pack: only a data read notices.
    let victim = packs
        .iter()
        .max_by_key(|p| fs::metadata(p).unwrap().len())
        .unwrap();
    let mut bytes = fs::read(victim).unwrap();
    let mid = bytes.len() / 3;
    bytes[mid] ^= 0x40;
    fs::set_permissions(victim, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(victim, &bytes).unwrap();
    let quick = check::check(&repo, false, &Progress::new(), &silent).unwrap();
    assert!(quick.ok(), "structure is still intact");
    let full = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(!full.ok());
    // A missing pack is a structural error.
    fs::remove_file(victim).unwrap();
    let quick = check::check(&repo, false, &Progress::new(), &silent).unwrap();
    assert!(
        quick.errors.iter().any(|m| m.contains("missing")),
        "{:?}",
        quick.errors
    );
}

#[test]
fn prune_reclaims_space_and_keeps_the_rest_intact() {
    let mut e = env();
    write(&e.src.join("keep.bin"), &noise(2 << 20, 10));
    write(&e.src.join("temp.bin"), &noise(4 << 20, 11));
    let repo = e.init(Some(PW));
    e.snap(&repo);
    fs::remove_file(e.src.join("temp.bin")).unwrap();
    let s2 = e.snap(&repo);
    let size = |r: &Path| all_repo_bytes(r).len();
    let before = size(&e.root);

    e.profile.retention = Retention {
        keep_last: 1,
        keep_within: None,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
        keep_yearly: 0,
        ..Retention::default()
    };
    let dry = gc::prune(&repo, &e.owner, &e.profile.retention, true, &silent).unwrap();
    assert_eq!(dry.removed.len(), 1);
    assert_eq!(
        repo.snapshots(&e.owner).unwrap().len(),
        2,
        "dry run removes nothing"
    );

    let r = gc::prune(&repo, &e.owner, &e.profile.retention, false, &silent).unwrap();
    assert_eq!(r.removed.len(), 1);
    let after = size(&e.root);
    assert!(before - after > 3 << 20, "freed {} bytes", before - after);
    assert!(r.gc.unwrap().bytes_freed > 3 << 20);

    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok() && report.warnings.is_empty(), "{report:?}");
    let out = e._tmp.path().join("r");
    e.restore_to(&repo, &s2, &out);
    assert!(fs::read(out.join("src/keep.bin")).unwrap() == noise(2 << 20, 10));
}

#[test]
fn orphan_packs_from_interrupted_backup_are_collected() {
    let e = env();
    write(&e.src.join("a"), &noise(1 << 20, 5));
    let repo = e.init(Some(PW));
    e.snap(&repo);
    // Simulate a pack written by a backup that died before writing its index.
    repo.backend()
        .write(
            &flux_vault::backend::pack_path(
                "ab00000000000000000000000000000000000000000000000000000000000000",
            ),
            b"junk",
        )
        .unwrap();
    let c = check::check(&repo, false, &Progress::new(), &silent).unwrap();
    assert!(c.ok() && c.warnings.len() == 1);
    let g = gc::prune(
        &repo,
        &e.owner,
        &Retention {
            enabled: false,
            ..Retention::default()
        },
        false,
        &silent,
    )
    .unwrap();
    assert_eq!(g.gc.unwrap().orphan_packs, 1);
    assert!(
        check::check(&repo, false, &Progress::new(), &silent)
            .unwrap()
            .warnings
            .is_empty()
    );
}

#[test]
fn unencrypted_vault_and_shared_dedup_between_owners() {
    let e = env();
    write(&e.src.join("shared.bin"), &noise(2 << 20, 9));
    let repo = e.init(None);
    assert!(!repo.config.encrypted);
    let s1 = e.snap(&repo);
    let reopened = VaultRepo::open(Box::new(LocalBackend::new(&e.root)), None).unwrap();
    // Another machine backing up the same file stores (almost) nothing new.
    let other = Owner {
        profile: "t".into(),
        host: "other-host".into(),
        user: "u".into(),
    };
    let out = backup(
        &reopened,
        &BackupRequest {
            owner: &other,
            profile: &e.profile,
            name: None,
            dry_run: false,
            never: vec![],
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    let BackupOutcome::Created { snapshot, .. } = out else {
        panic!()
    };
    assert!(snapshot.stats.stored_bytes < s1.stats.stored_bytes / 10);
    assert_eq!(
        reopened.snapshots(&e.owner).unwrap().len(),
        1,
        "owners see their own snapshots"
    );
    assert_eq!(reopened.snapshots(&other).unwrap().len(), 1);
}

/// Deleting a snapshot frees its data right away (reported bug: deleting the only snapshot
/// left all its data on the drive, and `check --read-data` still read it all).
#[test]
fn deleting_snapshots_frees_their_data() {
    let e = env();
    write(&e.src.join("big.bin"), &noise(3 << 20, 7));
    write(&e.src.join("small.txt"), b"hello");
    let repo = e.init(Some(PW));
    let first = e.snap(&repo);
    let packs = |r: &VaultRepo| r.backend().list("data").unwrap();
    assert!(!packs(&repo).is_empty());

    let freed = gc::delete(&repo, &[first.id.as_str()], &silent).unwrap();
    assert!(freed.bytes_freed >= 3 << 20, "{freed:?}");
    assert!(packs(&repo).is_empty(), "no data left: {:?}", packs(&repo));
    let c = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(c.ok(), "{:?}", c.errors);
    assert_eq!((c.snapshots, c.bytes_read), (0, 0));
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);

    // With two owners sharing the Vault, deleting one's snapshot keeps what the other uses.
    let a = e.snap(&repo);
    let other = Owner {
        profile: "other".into(),
        ..e.owner.clone()
    };
    let b = match backup(
        &repo,
        &BackupRequest {
            owner: &other,
            profile: &e.profile,
            name: None,
            dry_run: false,
            never: vec![e.root.clone()],
        },
        &Progress::new(),
        &silent,
    )
    .unwrap()
    {
        BackupOutcome::Created { snapshot, .. } => *snapshot,
        o => panic!("{o:?}"),
    };
    let freed = gc::delete(&repo, &[a.id.as_str()], &silent).unwrap();
    assert_eq!(
        freed.bytes_freed, 0,
        "everything is still used by the other owner"
    );
    let c = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(
        c.ok() && c.warnings.is_empty(),
        "{:?} {:?}",
        c.errors,
        c.warnings
    );
    let out = e._tmp.path().join("out");
    e.restore_to(&repo, &b, &out);
    assert_eq!(
        fs::read(out.join("src/big.bin")).unwrap(),
        noise(3 << 20, 7)
    );
}

/// Data left behind by older versions (records deleted without collecting) shows up in
/// `check` as a warning, and pruning removes it.
#[test]
fn check_reports_data_no_snapshot_uses() {
    let e = env();
    write(&e.src.join("big.bin"), &noise(1 << 20, 3));
    let repo = e.init(Some(PW));
    let s = e.snap(&repo);
    repo.delete_record(&s.id).unwrap(); // what deleting used to do
    let c = check::check(&repo, false, &Progress::new(), &silent).unwrap();
    assert!(c.ok());
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("belongs to no snapshot")),
        "{:?}",
        c.warnings
    );
    let r = gc::prune(&repo, &e.owner, &Retention::default(), false, &silent).unwrap();
    assert!(r.gc.unwrap().bytes_freed >= 1 << 20);
    assert!(repo.backend().list("data").unwrap().is_empty());
}
