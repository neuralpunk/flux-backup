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
    env_in(tempfile::tempdir().unwrap())
}

/// A snapshot records the metadata of its source's parent folders too. Tests that compare
/// two snapshots' trees use a parent no other test creates folders in (`/tmp` changes
/// whenever a parallel test makes or removes its temporary folder).
fn env_isolated(name: &str) -> Env {
    let base = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    env_in(tempfile::tempdir_in(&base).unwrap())
}

fn env_in(tmp: tempfile::TempDir) -> Env {
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
                cache_dir: Some(self._tmp.path().join("cache")),
                keep: Vec::new(),
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
                exact: None,
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

/// An encrypted Vault never gets an empty password: not when made, not as another key.
#[test]
fn an_empty_password_is_refused() {
    let e = env();
    let made = VaultRepo::init(
        Box::new(LocalBackend::new(&e.root)),
        Some(""),
        KdfParams::TESTING,
    );
    assert!(matches!(made, Err(VaultError::EmptyPassword)));
    assert!(!e.root.join("config").exists(), "nothing was created");
    let repo = e.init(Some(PW));
    assert!(matches!(
        repo.add_key("", "empty"),
        Err(VaultError::EmptyPassword)
    ));
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

/// A snapshot whose record can't be read is not a snapshot that doesn't exist: pruning
/// and deleting must not take its data for unused.
#[test]
fn gc_refuses_while_a_snapshot_record_is_unreadable() {
    let e = env();
    write(&e.src.join("a.bin"), &noise(1 << 20, 30));
    let repo = e.init(Some(PW));
    let s1 = e.snap(&repo);
    fs::remove_file(e.src.join("a.bin")).unwrap();
    write(&e.src.join("b.bin"), &noise(1 << 20, 31));
    let s2 = e.snap(&repo);

    // The first record becomes unreadable (as a failing read would leave it for gc).
    let rec = e.root.join("snapshots").join(&s1.id);
    let good = fs::read(&rec).unwrap();
    fs::write(&rec, b"not a record").unwrap();
    let packs = || fs::read_dir(e.root.join("data")).unwrap().count();
    let before = all_repo_bytes(&e.root).len();
    let n = packs();

    let err = gc::delete(&repo, &[&s2.id], &silent).unwrap_err();
    assert!(
        matches!(err, flux_vault::VaultError::UnreadableSnapshot(..)),
        "{err}"
    );
    assert_eq!(packs(), n, "no pack was deleted");
    let report = check::check(&repo, false, &Progress::new(), &silent).unwrap();
    assert!(
        report.errors.iter().any(|m| m.contains(&s1.id)),
        "{report:?}"
    );

    // Readable again: its data is all there, and gc only frees what the deleted one used.
    fs::write(&rec, &good).unwrap();
    let r = gc::delete(&repo, &[], &silent).unwrap();
    assert!(r.bytes_freed > 0);
    assert!(all_repo_bytes(&e.root).len() < before);
    let out = e._tmp.path().join("r");
    e.restore_to(&repo, &s1, &out);
    assert!(fs::read(out.join("src/a.bin")).unwrap() == noise(1 << 20, 30));

    // A record that stays damaged can be deleted by its id, and gc works again.
    fs::write(&rec, b"not a record").unwrap();
    assert!(repo.is_damaged_record(&s1.id));
    gc::delete(&repo, &[&s1.id], &silent).unwrap();
    assert!(repo.snapshots(&e.owner).unwrap().is_empty());
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
            cache_dir: None,
            keep: Vec::new(),
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
    let e = env_isolated("deleting");
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
            cache_dir: None,
            keep: Vec::new(),
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

/// Opening again with the same password reuses the unlocked key (no Argon2id), but never
/// lets a wrong password in, and a removed password stops working at once.
#[test]
fn reopening_reuses_the_unlocked_key_safely() {
    let e = env();
    let repo = e.init(Some(PW));
    let spare = repo.add_key("spare password", "spare").unwrap();
    let backend = || Box::new(LocalBackend::new(&e.root));
    // Unlock with each password, twice.
    for pw in [PW, "spare password", PW, "spare password"] {
        assert!(VaultRepo::open(backend(), Some(pw)).is_ok(), "{pw}");
    }
    assert!(matches!(
        VaultRepo::open(backend(), Some("wrong")),
        Err(VaultError::WrongPassword)
    ));
    // Remove the spare password: it no longer opens the Vault, cached or not.
    VaultRepo::open(backend(), Some(PW))
        .unwrap()
        .remove_key(&spare)
        .unwrap();
    assert!(matches!(
        VaultRepo::open(backend(), Some("spare password")),
        Err(VaultError::WrongPassword)
    ));
    assert!(VaultRepo::open(backend(), Some(PW)).is_ok());
}

/// A file several read-ahead batches long is chunked in parallel and restored byte for
/// byte, and an identical copy in the same backup is stored once.
#[test]
fn large_files_chunked_in_parallel_round_trip() {
    let e = env();
    let big = noise(70 << 20, 11);
    write(&e.src.join("disk.img"), &big);
    write(&e.src.join("copy/disk.img"), &big);
    let repo = e.init(Some(PW));
    let s = e.snap(&repo);
    assert!(
        s.stats.stored_bytes < (70 << 20) * 11 / 10,
        "stored {} for two identical 70 MiB files",
        s.stats.stored_bytes
    );
    let out = e._tmp.path().join("r");
    e.restore_to(&repo, &s, &out);
    assert!(fs::read(out.join("src/disk.img")).unwrap() == big);
    assert!(fs::read(out.join("src/copy/disk.img")).unwrap() == big);
    // Backing up the same content again reuses every chunk.
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(&e.src.join("again.img"), &big);
    let s2 = e.snap(&repo);
    assert!(
        s2.stats.stored_bytes < 1 << 20,
        "stored {}",
        s2.stats.stored_bytes
    );
}

/// The file list a backup leaves in the cache is exactly what rebuilding it from the
/// repository gives, and the next backup only uses it for that same snapshot.
#[test]
fn cached_file_list_matches_the_repository() {
    let e = env();
    sample(&e.src);
    let repo = e.init(Some(PW));
    let s1 = e.snap(&repo);
    let cache = e._tmp.path().join("cache");
    let file = fs::read_dir(&cache)
        .unwrap()
        .flatten()
        .next()
        .expect("a cached list")
        .path();
    let record = |id: &str| {
        repo.records()
            .unwrap()
            .into_iter()
            .find(|r| r.meta.id == id)
            .unwrap()
    };
    // What the cache holds is what flatten gives (checked through a second, unchanged run:
    // it must find nothing changed, so it must have read a correct list).
    let flat = repo.flatten(&record(&s1.id)).unwrap();
    assert!(!flat.is_empty());
    let cached =
        flux_vault::backup::cached_file_list(&cache, &repo, &e.owner, &record(&s1.id).tree)
            .expect("cached for the latest snapshot");
    assert!(
        cached == flat,
        "the cached list differs from the repository's"
    );
    match e.backup(&repo) {
        BackupOutcome::Unchanged { previous, .. } => assert_eq!(previous, s1.id),
        o => panic!("expected unchanged: {o:?}"),
    }
    // A change: the new snapshot is right whether or not the cache was used.
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(&e.src.join("docs/new.txt"), b"new");
    let s2 = e.snap(&repo);
    assert_eq!(s2.stats.new, 1, "{:?}", s2.stats);
    // A cache left from another snapshot is not used: damage it and back up again.
    fs::write(&file, b"not a cache").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(&e.src.join("docs/newer.txt"), b"newer");
    let s3 = e.snap(&repo);
    assert_eq!(s3.stats.new, 1, "{:?}", s3.stats);
    let out = e._tmp.path().join("r");
    e.restore_to(&repo, &s3, &out);
    assert_eq!(fs::read(out.join("src/docs/newer.txt")).unwrap(), b"newer");
}

/// A 48 MiB disk image with 12 KiB of data in three places (a hole at the start, between
/// and at the end). `None` when this filesystem doesn't make sparse files.
fn sparse_file(path: &Path, marker: u8) -> Option<()> {
    use std::os::unix::fs::FileExt;
    let f = fs::File::create(path).unwrap();
    f.set_len(48 << 20).unwrap();
    f.write_all_at(&[marker; 4096], 1 << 20).unwrap();
    f.write_all_at(&noise(4096, u64::from(marker)), 20 << 20)
        .unwrap();
    f.write_all_at(&[marker ^ 0xff; 4096], 40 << 20).unwrap();
    drop(f);
    flux_fs::fsops::is_sparse(&fs::metadata(path).unwrap()).then_some(())
}

fn allocated(path: &Path) -> u64 {
    fs::metadata(path).unwrap().blocks() * 512
}

/// The Vault config as an older Flux wrote it: format 1.
fn make_format_1(root: &Path) {
    let path = root.join("config");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["format"] = 1.into();
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

fn record_json(root: &Path, repo: &VaultRepo, id: &str) -> serde_json::Value {
    let sealed = fs::read(root.join("snapshots").join(id)).unwrap();
    let plain = repo.crypto.open(&sealed).unwrap();
    serde_json::from_slice(&plain).unwrap()
}

#[test]
fn sparse_files_keep_their_holes() {
    let e = env();
    if sparse_file(&e.src.join("disk.img"), 1).is_none() {
        eprintln!("this filesystem doesn't make sparse files; skipping");
        return;
    }
    write(&e.src.join("plain.txt"), b"dense");
    let repo = e.init(Some(PW));
    assert_eq!(repo.tree_format(), 2, "new Vaults are format 2");
    let s = e.snap(&repo);
    assert_eq!(s.format, 2);
    assert_eq!(record_json(&e.root, &repo, &s.id)["tree_format"], 2);
    // Only the data was read; the file's full length is recorded.
    assert!(
        s.stats.copied_bytes < 1 << 20,
        "read {}",
        s.stats.copied_bytes
    );
    let listed = flux_vault::entries_of(&repo, &s.id).unwrap();
    let img = listed
        .iter()
        .find(|x| x.path.ends_with(b"disk.img"))
        .unwrap();
    assert_eq!(img.size, 48 << 20);

    let out = e._tmp.path().join("restored");
    let r = e.restore_to(&repo, &s, &out);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_same_tree(&e.src, &out.join("src"));
    let restored = out.join("src/disk.img");
    assert_eq!(fs::metadata(&restored).unwrap().len(), 48 << 20);
    assert!(
        allocated(&restored) < 4 << 20,
        "restored sparse: {} bytes allocated",
        allocated(&restored)
    );

    // Unchanged on the next run (its content, holes and all, comes from the parent).
    assert!(matches!(e.backup(&repo), BackupOutcome::Unchanged { .. }));
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn format_1_vaults_stay_format_1_until_upgraded() {
    let e = env();
    if sparse_file(&e.src.join("disk.img"), 1).is_none() {
        eprintln!("this filesystem doesn't make sparse files; skipping");
        return;
    }
    let repo = e.init(Some(PW));
    drop(repo);
    make_format_1(&e.root);

    // An older Vault: written densely, in format 1, as Flux 0.3 would.
    let repo = e.open();
    assert_eq!(repo.tree_format(), 1);
    let old = e.snap(&repo);
    assert_eq!(old.format, 1);
    assert!(
        record_json(&e.root, &repo, &old.id)
            .get("tree_format")
            .is_none(),
        "format 1 records look as they always did"
    );
    assert_eq!(old.stats.copied_bytes, 48 << 20, "read densely");
    let out = e._tmp.path().join("old");
    e.restore_to(&repo, &old, &out);
    assert_same_tree(&e.src, &out.join("src"));

    // Upgraded: a new snapshot of a changed image keeps holes, the old one still reads.
    drop(repo);
    let backend = LocalBackend::new(&e.root);
    let u = VaultRepo::upgrade_format(&backend).unwrap();
    assert_eq!((u.from, u.to), (1, 2));
    assert_eq!(
        VaultRepo::upgrade_format(&backend).unwrap().from,
        2,
        "once only"
    );
    let repo = e.open();
    assert_eq!(repo.tree_format(), 2);
    fs::remove_file(e.src.join("disk.img")).unwrap();
    sparse_file(&e.src.join("disk.img"), 9).unwrap();
    let new = e.snap(&repo);
    assert_eq!(record_json(&e.root, &repo, &new.id)["tree_format"], 2);
    assert!(new.stats.copied_bytes < 1 << 20);

    let out_new = e._tmp.path().join("new");
    e.restore_to(&repo, &new, &out_new);
    assert_same_tree(&e.src, &out_new.join("src"));
    assert!(allocated(&out_new.join("src/disk.img")) < 4 << 20);
    let out_old = e._tmp.path().join("old-again");
    e.restore_to(&repo, &old, &out_old);
    assert_eq!(
        fs::read(out_old.join("src/disk.img")).unwrap(),
        fs::read(out.join("src/disk.img")).unwrap(),
        "the format 1 snapshot is still read as before"
    );
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);

    // Pruning walks both formats' trees: deleting the old snapshot keeps the new intact.
    gc::delete(&repo, &[old.id.as_str()], &silent).unwrap();
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn a_newer_format_is_refused_in_words() {
    let e = env();
    drop(e.init(Some(PW)));
    let path = e.root.join("config");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["format"] = 99.into();
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let err = VaultRepo::open(Box::new(LocalBackend::new(&e.root)), Some(PW))
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("newer than this Flux supports"), "{err}");
    assert!(VaultRepo::upgrade_format(&LocalBackend::new(&e.root)).is_err());
}

#[test]
fn a_locked_vault_says_who_holds_it() {
    let e = env();
    let repo = e.init(Some(PW));
    let held = repo.lock(true).unwrap();
    let other = e.open();
    let err = other.lock(false).err().unwrap().to_string();
    assert!(err.contains("another Flux job is using the Vault"), "{err}");
    assert!(
        err.contains(&format!("process {}", std::process::id())),
        "{err}"
    );
    assert!(err.contains("flux unlock"), "{err}");
    // Its own live lock isn't stale, so clearing without force leaves it.
    assert!(other.clear_locks(false).unwrap().is_empty());
    drop(held);
    assert!(other.lock(true).is_ok());
}

/// "Hide file sizes": every pack is padded to its Padmé length, and everything still
/// restores and checks.
#[test]
fn padded_packs_hide_sizes() {
    let e = env();
    write(&e.src.join("small.txt"), b"a short secret");
    write(&e.src.join("big.bin"), &noise(3 << 20, 5));
    let mut repo = e.init(Some(PW));
    repo.pad_packs = true;
    let s = e.snap(&repo);
    let packs: Vec<PathBuf> = fs::read_dir(e.root.join("data"))
        .unwrap()
        .flat_map(|d| {
            fs::read_dir(d.unwrap().path())
                .unwrap()
                .map(|f| f.unwrap().path())
        })
        .collect();
    assert!(!packs.is_empty());
    for p in &packs {
        let len = fs::metadata(p).unwrap().len();
        assert_eq!(
            flux_vault::pack::padme(len),
            len,
            "{} is padded",
            p.display()
        );
    }
    let out = e._tmp.path().join("restored");
    let r = e.restore_to(&repo, &s, &out);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_same_tree(&e.src, &out.join("src"));
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);
    // A reader that knows nothing of padding (a fresh open) reads it too.
    drop(repo);
    let repo = e.open();
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);
}

/// A source whose drive isn't connected (`keep`): the Vault snapshot keeps its files from
/// the previous one without reading anything, and nothing counts as deleted.
#[test]
fn missing_drive_keeps_its_files() {
    let mut e = env();
    let photos = e._tmp.path().join("photos");
    fs::create_dir_all(&photos).unwrap();
    write(&photos.join("a.jpg"), &noise(100_000, 9));
    write(&e.src.join("notes.txt"), b"v1");
    e.profile.sources.paths = vec![e.src.clone(), photos.clone()];
    let repo = e.init(Some(PW));
    let first = e.snap(&repo);

    write(&e.src.join("notes.txt"), b"v2 changed");
    fs::remove_dir_all(&photos).unwrap(); // gone, as an unplugged drive's folder is
    let mut reduced = e.profile.clone();
    reduced.sources.paths = vec![e.src.clone()];
    let out = backup(
        &repo,
        &BackupRequest {
            owner: &e.owner,
            profile: &reduced,
            name: None,
            dry_run: false,
            never: vec![e.root.clone()],
            cache_dir: Some(e._tmp.path().join("cache")),
            keep: vec![photos.clone()],
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    let BackupOutcome::Created {
        snapshot: second, ..
    } = out
    else {
        panic!("expected a snapshot")
    };
    assert_eq!(second.stats.deleted, 0);
    assert!(
        second.stats.copied_bytes < 100,
        "the photo wasn't read: {}",
        second.stats.copied_bytes
    );
    assert_eq!(
        second.sources_missing[0].kept_from.as_deref(),
        Some(first.id.as_str())
    );
    let listed = flux_vault::entries_of(&repo, &second.id).unwrap();
    assert!(listed.iter().any(|x| x.path.ends_with(b"photos/a.jpg")));

    // Restoring the kept file gives the photo back.
    let out = e._tmp.path().join("restored");
    let source = VaultSource::new(&repo, &second.id).unwrap();
    let r = restore(
        &source,
        &RestoreRequest {
            snapshot: &second,
            paths: vec![photos.clone()],
            to: Some(out.clone()),
            keep_existing: true,
            dry_run: false,
            xattrs: XattrPolicy {
                acl: true,
                other: true,
            },
            links: flux_fs::safe::LinkPolicy::for_process(),
            exact: None,
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(
        fs::read(out.join("photos/a.jpg")).unwrap(),
        noise(100_000, 9)
    );
}

/// A replica: the same Vault in another place, copied file by file. It opens with the same
/// password and restores; later runs copy only what's new and remove what was pruned.
#[test]
fn replicas_copy_the_vault() {
    let e = env();
    write(&e.src.join("a.txt"), b"first");
    write(&e.src.join("big.bin"), &noise(3 << 20, 4));
    let repo = e.init(Some(PW));
    let s1 = e.snap(&repo);
    let copy_root = e._tmp.path().join("usb/flux-vault");
    let replica = LocalBackend::new(&copy_root);
    let r = flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert!(r.copied_files > 3 && r.snapshots == 1, "{r:?}");

    // The replica is the same Vault: the same password opens it, and it restores.
    let copy = VaultRepo::open(Box::new(LocalBackend::new(&copy_root)), Some(PW)).unwrap();
    assert_eq!(copy.config.id, repo.config.id);
    let out = e._tmp.path().join("from-copy");
    let source = VaultSource::new(&copy, &s1.id).unwrap();
    restore(
        &source,
        &RestoreRequest {
            snapshot: &s1,
            paths: vec![e.src.clone()],
            to: Some(out.clone()),
            keep_existing: true,
            dry_run: false,
            xattrs: XattrPolicy {
                acl: true,
                other: true,
            },
            links: flux_fs::safe::LinkPolicy::for_process(),
            exact: None,
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert_same_tree(&e.src, &out.join("src"));

    // Nothing new: nothing copied. A small change: little copied.
    let again =
        flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert_eq!((again.copied_files, again.removed_files), (0, 0));
    write(&e.src.join("a.txt"), b"second");
    let s2 = e.snap(&repo);
    let r = flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert!(r.copied_bytes < 1 << 20, "only the change: {r:?}");
    assert_eq!(r.snapshots, 2);

    // A note changes a snapshot record: copied. Deleting a snapshot at the source: gone
    // from the replica too, and it still checks out.
    let mut rec = repo.read_record(&s2.id).unwrap();
    rec.meta.note = Some("noted".into());
    repo.write_record(&rec).unwrap();
    gc::delete(&repo, &[s1.id.as_str()], &silent).unwrap();
    let r = flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert!(r.removed_files > 0 && r.snapshots == 1, "{r:?}");
    let copy = VaultRepo::open(Box::new(LocalBackend::new(&copy_root)), Some(PW)).unwrap();
    assert_eq!(
        copy.read_record(&s2.id).unwrap().meta.note.as_deref(),
        Some("noted")
    );
    let report = check::check(&copy, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{:?}", report.errors);

    // Another Vault already there is never overwritten.
    let other = e._tmp.path().join("other/flux-vault");
    VaultRepo::init(
        Box::new(LocalBackend::new(&other)),
        Some(PW),
        KdfParams::TESTING,
    )
    .unwrap();
    let err = flux_vault::replicate::replicate(
        repo.backend(),
        &LocalBackend::new(&other),
        &Progress::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("another Vault is already at"), "{err}");
}

/// A Vault "on a server": the local backend, saying every read is a round trip, and
/// counting them.
struct Far {
    inner: LocalBackend,
    whole: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ranges: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl flux_vault::backend::Backend for Far {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if path.starts_with("data/") {
            self.whole
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.inner.read(path)
    }
    fn read_range(&self, path: &str, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.ranges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.read_range(path, offset, len)
    }
    fn remote(&self) -> bool {
        true
    }
    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.inner.write(path, data)
    }
    fn list(&self, dir: &str) -> std::io::Result<Vec<(String, u64)>> {
        self.inner.list(dir)
    }
    fn remove(&self, path: &str) -> std::io::Result<()> {
        self.inner.remove(path)
    }
    fn exists(&self, path: &str) -> std::io::Result<bool> {
        self.inner.exists(path)
    }
    fn lock(&self, exclusive: bool) -> std::io::Result<Option<flux_vault::backend::LockGuard>> {
        self.inner.lock(exclusive)
    }
    fn lock_holders(&self) -> std::io::Result<Vec<flux_transport::lock::LockInfo>> {
        self.inner.lock_holders()
    }
    fn clear_locks(&self, force: bool) -> std::io::Result<Vec<flux_transport::lock::LockInfo>> {
        self.inner.clear_locks(force)
    }
}

/// Restoring from a Vault on a server: a thousand small files in many folders must not
/// cost a round trip (three, in truth) for each file and each folder. Their packs are
/// fetched whole, a few of them, and everything else comes from memory.
#[test]
fn a_restore_from_a_server_fetches_packs_not_blobs() {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    let e = env();
    for d in 0..50 {
        for f in 0..20 {
            write(
                &e.src.join(format!("d{d:02}/f{f:02}.txt")),
                format!("file {f} of folder {d}, and some words to tell them apart").as_bytes(),
            );
        }
    }
    write(&e.src.join("big.bin"), &noise(6 << 20, 3));
    let near = e.init(Some(PW));
    let snap = e.snap(&near);
    drop(near);

    let (whole, ranges) = (
        std::sync::Arc::new(AtomicU64::new(0)),
        std::sync::Arc::new(AtomicU64::new(0)),
    );
    let far = VaultRepo::open(
        Box::new(Far {
            inner: LocalBackend::new(&e.root),
            whole: whole.clone(),
            ranges: ranges.clone(),
        }),
        Some(PW),
    )
    .unwrap();
    let packs = far.backend().list("data").unwrap().len() as u64;
    let to = e._tmp.path().join("restored");
    let out = e.restore_to(&far, &snap, &to);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let back = to.join("src");
    assert_eq!(
        fs::read_to_string(back.join("d49/f19.txt")).unwrap(),
        "file 19 of folder 49, and some words to tell them apart"
    );
    assert!(fs::read(back.join("big.bin")).unwrap() == noise(6 << 20, 3));
    // One file back: the folders on the way to it are read, not all 51 (nor a pack
    // fetched whole for it).
    let far = VaultRepo::open(
        Box::new(Far {
            inner: LocalBackend::new(&e.root),
            whole: whole.clone(),
            ranges: ranges.clone(),
        }),
        Some(PW),
    )
    .unwrap();
    let (w0, r0) = (whole.load(Relaxed), ranges.load(Relaxed));
    let one = e.src.join("d07/f03.txt");
    let source = VaultSource::for_paths(&far, &snap.id, std::slice::from_ref(&one)).unwrap();
    let to1 = e._tmp.path().join("one");
    let out = restore(
        &source,
        &RestoreRequest {
            snapshot: &snap,
            paths: vec![one],
            to: Some(to1.clone()),
            keep_existing: true,
            dry_run: false,
            xattrs: XattrPolicy {
                acl: false,
                other: false,
            },
            links: flux_fs::safe::LinkPolicy::for_process(),
            exact: None,
        },
        &Progress::new(),
        &silent,
    )
    .unwrap();
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(
        fs::read_to_string(to1.join("f03.txt")).unwrap(),
        "file 3 of folder 7, and some words to tell them apart"
    );
    let reads = whole.load(Relaxed) - w0 + ranges.load(Relaxed) - r0;
    let depth = e.src.components().count() as u64;
    assert!(
        reads <= depth + 6,
        "{reads} reads for one file, {depth} folders deep"
    );
    let (whole, ranges) = (
        std::sync::Arc::new(AtomicU64::new(w0)),
        std::sync::Arc::new(AtomicU64::new(r0)),
    );
    // 1000 files, 51 folders and a 6 MiB file: far over a thousand blobs.
    let (w, r) = (whole.load(Relaxed), ranges.load(Relaxed));
    assert!(w <= packs, "{w} whole packs fetched, of {packs}");
    assert!(
        r <= 2 * packs + 8,
        "{r} single blobs fetched, {w} whole packs (of {packs})"
    );
}
