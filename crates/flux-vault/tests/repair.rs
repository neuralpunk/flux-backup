//! Repair data, end to end: Vaults written with parity, damaged on disk the ways drives
//! damage files, and put right.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flux_core::profile::{Profile, Target, VaultOptions};
use flux_core::progress::{Progress, silent};
use flux_fs::outcome::BackupOutcome;
use flux_transport::lock::LockInfo;
use flux_vault::backend::{Backend, LockGuard};
use flux_vault::backup::{BackupRequest, backup};
use flux_vault::check::{CheckOptions, CheckReport, check, check_with};
use flux_vault::crypto::KdfParams;
use flux_vault::protect::{self, ParityState};
use flux_vault::{LocalBackend, Owner, VaultRepo, gc};

const PW: &str = "correct horse battery staple";

struct Env {
    tmp: tempfile::TempDir,
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
    Env {
        src,
        root: flux_vault::local_root(&target),
        profile,
        owner: Owner {
            profile: "t".into(),
            host: "h".into(),
            user: "u".into(),
        },
        tmp,
    }
}

impl Env {
    /// A new Vault that keeps `parity` percent of repair data.
    fn init(&self, password: Option<&str>, parity: u8) -> VaultRepo {
        let mut repo = VaultRepo::init(
            Box::new(LocalBackend::new(&self.root)),
            password,
            KdfParams::TESTING,
        )
        .unwrap();
        repo.parity = parity;
        repo
    }
    fn open(&self, parity: u8) -> VaultRepo {
        let mut repo = VaultRepo::open(Box::new(LocalBackend::new(&self.root)), Some(PW)).unwrap();
        repo.parity = parity;
        repo
    }
    fn snap(&self, repo: &VaultRepo) -> String {
        let out = backup(
            repo,
            &BackupRequest {
                owner: &self.owner,
                profile: &self.profile,
                name: None,
                dry_run: false,
                never: vec![self.root.clone()],
                cache_dir: Some(self.tmp.path().join("cache")),
                keep: Vec::new(),
            },
            &Progress::new(),
            &silent,
        )
        .unwrap();
        match out {
            BackupOutcome::Created { snapshot, .. } => snapshot.id,
            o => panic!("expected a snapshot: {o:?}"),
        }
    }
    /// A backup that may find nothing changed.
    fn snap_or_unchanged(&self, repo: &VaultRepo) {
        backup(
            repo,
            &BackupRequest {
                owner: &self.owner,
                profile: &self.profile,
                name: None,
                dry_run: false,
                never: vec![self.root.clone()],
                cache_dir: Some(self.tmp.path().join("cache")),
                keep: Vec::new(),
            },
            &Progress::new(),
            &silent,
        )
        .unwrap();
    }
    /// Files under a directory of the Vault, two levels deep for `data`.
    fn files(&self, dir: &str) -> Vec<PathBuf> {
        fn walk(p: &Path, out: &mut Vec<PathBuf>) {
            let Ok(rd) = fs::read_dir(p) else { return };
            for e in rd {
                let e = e.unwrap().path();
                if e.is_dir() {
                    walk(&e, out);
                } else {
                    out.push(e);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root.join(dir), &mut out);
        out.sort();
        out
    }
    fn biggest_pack(&self) -> PathBuf {
        self.files("data")
            .into_iter()
            .max_by_key(|p| fs::metadata(p).unwrap().len())
            .unwrap()
    }
    /// Every file's bytes as a restore reads them.
    fn read_all(&self, repo: &VaultRepo, snapshot: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
        let rec = repo.read_record(snapshot).unwrap();
        let index = repo.index().unwrap();
        repo.flatten(&rec)
            .unwrap()
            .into_iter()
            .filter(|(e, _)| e.kind == flux_fs::entry::Kind::File)
            .map(|(e, content)| {
                let mut data = Vec::new();
                for b in &content.blobs {
                    let loc = index.get(b).expect("indexed");
                    data.extend(
                        flux_vault::pack::read_blob(repo.backend(), &repo.crypto, loc).unwrap(),
                    );
                }
                (e.path, data)
            })
            .collect()
    }
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Change a file in place (Vault files are read-only).
fn damage(path: &Path, f: impl FnOnce(&mut Vec<u8>)) {
    let mut bytes = fs::read(path).unwrap();
    f(&mut bytes);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(path, &bytes).unwrap();
}

fn full(repo: &VaultRepo) -> CheckReport {
    check(repo, true, &Progress::new(), &silent).unwrap()
}

fn repair(repo: &VaultRepo) -> CheckReport {
    check_with(
        repo,
        CheckOptions {
            read_data: true,
            repair: true,
        },
        &Progress::new(),
        &silent,
    )
    .unwrap()
}

fn parity_of(e: &Env, file: &Path) -> PathBuf {
    e.root
        .join("parity")
        .join(file.strip_prefix(&e.root).unwrap())
}

#[test]
fn damaged_packs_are_rebuilt() {
    let e = env();
    for i in 0..4 {
        fs::write(e.src.join(format!("f{i}")), noise(5 << 20, i + 1)).unwrap();
    }
    let repo = e.init(Some(PW), 5);
    let snap = e.snap(&repo);
    let before = e.read_all(&repo, &snap);
    assert_eq!(before.len(), 4);

    // Everything written has its parity file: packs, the index, the snapshot record.
    for dir in ["data", "index", "snapshots"] {
        let files = e.files(dir);
        assert!(!files.is_empty());
        for f in &files {
            assert!(parity_of(&e, f).is_file(), "{f:?} has no repair data");
        }
    }
    let cover = protect::coverage(repo.backend()).unwrap();
    assert_eq!(cover.packs, cover.protected);
    // About 5%, and a little for the hashes.
    let share = cover.parity_bytes as f64 / cover.bytes as f64;
    assert!((0.05..0.07).contains(&share), "{share}");
    assert!(full(&repo).ok());

    // A flipped bit, a bad sector (4 KiB of zeros) and a 300 KiB run of garbage, in one
    // pack: more than a megabyte apart.
    let victim = e.biggest_pack();
    let original = fs::read(&victim).unwrap();
    damage(&victim, |b| {
        b[1_000_003] ^= 0x04;
        b[3_000_000..3_004_096].fill(0);
        let junk = noise(300 << 10, 99);
        b[7_000_000..7_000_000 + junk.len()].copy_from_slice(&junk);
    });
    // Without repairing: found, and said to be repairable; nothing is written.
    let found = full(&repo);
    assert_eq!((found.errors.len(), found.repairable), (1, 1), "{found:?}");
    assert!(found.errors[0].contains("its repair data can rebuild it"));
    assert!(fs::read(&victim).unwrap() != original);

    // Repairing: the pack is byte for byte what it was, and the Vault checks out.
    let fixed = repair(&repo);
    assert!(fixed.ok(), "{:?}", fixed.errors);
    assert_eq!(fixed.repaired.len(), 1, "{:?}", fixed.repaired);
    // 1 block, 2 (the sector straddles two) and 76.
    assert!(
        fixed.repaired[0].contains("repaired (79 blocks)"),
        "{:?}",
        fixed.repaired
    );
    assert!(fs::read(&victim).unwrap() == original);
    let after = full(&repo);
    assert!(after.ok() && after.repaired.is_empty() && after.warnings.is_empty());
    assert!(e.read_all(&repo, &snap) == before);

    // Too much: half the pack gone. What still reads of it is moved to a new pack, the
    // rest leaves the index, and the check says which files that costs.
    damage(&victim, |b| b.truncate(b.len() / 2));
    let lost = repair(&repo);
    assert!(!lost.ok(), "{lost:?}");
    assert!(
        lost.repaired.len() == 1 && lost.repaired[0].contains("taken out of the index"),
        "{:?}",
        lost.repaired
    );
    assert!(
        lost.errors.iter().all(|m| m.contains("is missing")),
        "{:?}",
        lost.errors
    );
    assert!(!victim.exists() && !parity_of(&e, &victim).exists());

    // The files are still there: the next backup stores what was lost again, and the
    // snapshot from before the damage reads back whole too.
    let again = e.snap(&repo);
    let healed = full(&repo);
    assert!(healed.ok(), "{:?}", healed.errors);
    assert!(e.read_all(&repo, &snap) == before);
    assert!(e.read_all(&repo, &again) == before);
}

#[test]
fn the_index_a_snapshot_the_config_and_a_key_are_rebuilt() {
    let e = env();
    fs::write(e.src.join("a"), noise(300_000, 1)).unwrap();
    let repo = e.init(Some(PW), 10);
    let snap = e.snap(&repo);
    // The config and the first key were written before the Vault was told to keep
    // repair data: upkeep adds theirs.
    let tidied = protect::tidy(&repo).unwrap();
    assert_eq!((tidied.added, tidied.removed), (2, 0));
    let config = e.root.join("config");
    let key = e.files("keys").remove(0);
    assert!(parity_of(&e, &config).is_file() && parity_of(&e, &key).is_file());

    // An index file and the snapshot record, each with a damaged byte.
    let index = e.files("index").remove(0);
    let record = e.files("snapshots").remove(0);
    let (index_was, record_was) = (fs::read(&index).unwrap(), fs::read(&record).unwrap());
    damage(&index, |b| b[40] ^= 1);
    damage(&record, |b| {
        let n = b.len();
        b[n - 5] ^= 0x80
    });
    // A plain check says what is wrong and that it can be repaired.
    let found = check(&repo, false, &Progress::new(), &silent).unwrap();
    assert_eq!(found.repairable, 2, "{found:?}");
    let fixed = repair(&repo);
    assert!(fixed.ok(), "{:?}", fixed.errors);
    assert_eq!(fixed.repaired.len(), 2, "{:?}", fixed.repaired);
    assert!(fs::read(&index).unwrap() == index_was && fs::read(&record).unwrap() == record_was);
    assert_eq!(repo.read_record(&snap).unwrap().meta.id, snap);

    // The config (its Vault id) and the only key: the Vault can't even be opened…
    let (config_was, key_was) = (fs::read(&config).unwrap(), fs::read(&key).unwrap());
    damage(&config, |b| {
        let at = b.windows(5).position(|w| w == b"\"id\":").unwrap() + 9;
        b[at] = if b[at] == b'0' { b'1' } else { b'0' };
    });
    damage(&key, |b| {
        let at = b.windows(12).position(|w| w == b"\"ciphertext\"").unwrap() + 20;
        b[at] = if b[at] == b'A' { b'B' } else { b'A' };
    });
    drop(repo);
    assert!(VaultRepo::open(Box::new(LocalBackend::new(&e.root)), Some(PW)).is_err());
    // …until they are repaired, which needs no password.
    let fixed = protect::repair_unopened(&LocalBackend::new(&e.root)).unwrap();
    assert_eq!(fixed.len(), 2, "{fixed:?}");
    assert!(fs::read(&config).unwrap() == config_was && fs::read(&key).unwrap() == key_was);
    let repo = e.open(10);
    assert!(full(&repo).ok());
    // Nothing more to do the second time.
    assert!(
        protect::repair_unopened(&LocalBackend::new(&e.root))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn damaged_repair_data_is_written_again() {
    let e = env();
    fs::write(e.src.join("a"), noise(2 << 20, 1)).unwrap();
    let repo = e.init(Some(PW), 5);
    e.snap(&repo);
    let pack = e.biggest_pack();
    let par = parity_of(&e, &pack);
    let par_was = fs::read(&par).unwrap();
    // In a parity block, and in the first copy of the head.
    damage(&par, |b| {
        let n = b.len();
        b[n / 2] ^= 1;
        b[30] ^= 1;
    });
    let found = full(&repo);
    assert!(found.ok(), "the data is fine: {:?}", found.errors);
    assert_eq!(found.warnings.len(), 1, "{:?}", found.warnings);
    assert!(found.warnings[0].contains("repair data of pack"));
    let fixed = repair(&repo);
    assert!(fixed.ok() && fixed.repaired.len() == 1, "{fixed:?}");
    assert!(fs::read(&par).unwrap() == par_was, "the same parity again");

    // Damage to the pack and to its parity at once, within what's left of the parity.
    damage(&par, |b| {
        let n = b.len();
        b[n / 2] ^= 1
    });
    let pack_was = fs::read(&pack).unwrap();
    damage(&pack, |b| b[5_000..9_000].fill(0xee));
    let fixed = repair(&repo);
    assert!(fixed.ok(), "{:?}", fixed.errors);
    assert!(fs::read(&pack).unwrap() == pack_was);
    assert_eq!(
        protect::parity_state(&pack_was, &fs::read(&par).unwrap()),
        ParityState::Sound
    );
}

#[test]
fn a_vault_without_repair_data_gets_it() {
    let e = env();
    for i in 0..3 {
        fs::write(e.src.join(format!("f{i}")), noise(1 << 20, i + 1)).unwrap();
    }
    // Written by a Flux that keeps none (or with the setting off).
    let repo = e.init(Some(PW), 0);
    e.snap(&repo);
    assert!(!e.root.join("parity").exists());
    assert_eq!(protect::coverage(repo.backend()).unwrap().protected, 0);
    // A repairing check with the setting off adds nothing.
    assert_eq!(repair(&repo).parity_added, 0);
    assert!(!e.root.join("parity").exists());
    drop(repo);

    // Turned on: new data has it at once, the rest when every byte is verified.
    let repo = e.open(5);
    fs::write(e.src.join("new"), noise(1 << 20, 9)).unwrap();
    e.snap(&repo);
    let cover = protect::coverage(repo.backend()).unwrap();
    assert!(
        cover.protected >= 1 && cover.protected < cover.packs,
        "{cover:?}"
    );
    let plain = full(&repo);
    assert!(
        plain.ok() && plain.parity_added == 0,
        "a plain check writes nothing"
    );
    let fixed = repair(&repo);
    assert!(fixed.ok() && fixed.parity_added > 0, "{fixed:?}");
    let cover = protect::coverage(repo.backend()).unwrap();
    assert_eq!(cover.protected, cover.packs);
    for dir in ["data", "index", "snapshots", "keys"] {
        for f in e.files(dir) {
            assert!(parity_of(&e, &f).is_file(), "{f:?}");
        }
    }
    assert!(e.root.join("parity/config").is_file());
    assert_eq!(repair(&repo).parity_added, 0, "all there now");
}

#[test]
fn pruning_and_removing_keys_take_the_repair_data_along() {
    let e = env();
    fs::write(e.src.join("a"), noise(3 << 20, 1)).unwrap();
    let repo = e.init(Some(PW), 5);
    let first = e.snap(&repo);
    fs::write(e.src.join("a"), noise(3 << 20, 2)).unwrap();
    let second = e.snap(&repo);
    let packs_before = e.files("data").len();
    gc::delete(&repo, &[first.as_str()], &silent).unwrap();
    assert!(
        e.files("data").len() < packs_before,
        "the first snapshot's data is gone"
    );
    // No parity file without its file, and none missing.
    for dir in ["data", "index", "snapshots", "keys"] {
        let names = |files: Vec<PathBuf>| -> Vec<String> {
            files
                .iter()
                .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names(e.files(dir)),
            names(e.files(&format!("parity/{dir}"))),
            "{dir}"
        );
    }
    assert!(full(&repo).ok());
    assert_eq!(repo.read_record(&second).unwrap().meta.id, second);

    // A removed password's key slot can't be rebuilt from repair data left behind.
    let added = repo.add_key("another password", "second").unwrap();
    let slot = e.root.join("keys").join(&added);
    assert!(parity_of(&e, &slot).is_file());
    repo.remove_key(&added).unwrap();
    assert!(!slot.exists() && !parity_of(&e, &slot).exists());
}

#[test]
fn a_sound_file_rewritten_since_is_never_repaired_into_its_old_self() {
    let e = env();
    fs::write(e.src.join("a"), noise(100_000, 1)).unwrap();
    let repo = e.init(Some(PW), 5);
    let snap = e.snap(&repo);
    let record = e.files("snapshots").remove(0);
    // A Flux that keeps no repair data names the snapshot: its parity is now stale.
    let old = e.open(0);
    let mut rec = old.read_record(&snap).unwrap();
    rec.meta.name = Some("before the move".into());
    old.write_record(&rec).unwrap();
    let renamed = fs::read(&record).unwrap();

    // The record is sound, so nothing is "repaired"; a plain check notes the stale
    // parity, a repairing one writes it again.
    let found = full(&repo);
    assert!(found.ok() && found.repairable == 0, "{found:?}");
    assert!(
        found.warnings.iter().any(|w| w.contains("out of date")),
        "{:?}",
        found.warnings
    );
    assert!(fs::read(&record).unwrap() == renamed);
    let fixed = repair(&repo);
    assert!(fixed.ok() && fixed.repaired.len() == 1, "{fixed:?}");
    assert!(
        fs::read(&record).unwrap() == renamed,
        "still the renamed record"
    );
    assert_eq!(
        repo.read_record(&snap).unwrap().meta.name.as_deref(),
        Some("before the move")
    );
    assert!(full(&repo).warnings.is_empty());

    // Stale again, and this time the record is damaged too. What is rebuilt is the
    // record as it was when its repair data was last written: the last change is lost,
    // the snapshot is not.
    rec.meta.name = Some("renamed again".into());
    old.write_record(&rec).unwrap();
    damage(&record, |b| b[30] ^= 1);
    assert!(repo.read_record(&snap).is_err());
    let fixed = repair(&repo);
    assert!(fixed.ok() && fixed.repaired.len() == 1, "{fixed:?}");
    assert!(fs::read(&record).unwrap() == renamed);
    assert_eq!(
        repo.read_record(&snap).unwrap().meta.name.as_deref(),
        Some("before the move")
    );
}

#[test]
fn copies_carry_the_repair_data_and_unencrypted_vaults_repair_too() {
    let e = env();
    fs::write(e.src.join("a"), noise(2 << 20, 1)).unwrap();
    // Not encrypted: a pack is still known by its hash.
    let mut repo = VaultRepo::init(
        Box::new(LocalBackend::new(&e.root)),
        None,
        KdfParams::TESTING,
    )
    .unwrap();
    repo.parity = 20;
    e.snap(&repo);
    protect::tidy(&repo).unwrap();

    let copy_root = e.tmp.path().join("copy/flux-vault");
    let replica = LocalBackend::new(&copy_root);
    let r = flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert!(r.copied_files > 0);
    let files = |root: &Path| -> Vec<PathBuf> {
        let mut all = Vec::new();
        let mut stack = vec![root.join("parity")];
        while let Some(d) = stack.pop() {
            for f in fs::read_dir(&d).unwrap() {
                let f = f.unwrap().path();
                if f.is_dir() {
                    stack.push(f);
                } else {
                    all.push(f.strip_prefix(root).unwrap().to_owned());
                }
            }
        }
        all.sort();
        all
    };
    assert_eq!(files(&e.root), files(&copy_root));
    assert!(files(&copy_root).len() >= 4);
    let again =
        flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    assert_eq!((again.copied_files, again.removed_files), (0, 0));

    // Damage in the copy is repaired in the copy.
    let mut copy = VaultRepo::open(Box::new(LocalBackend::new(&copy_root)), None).unwrap();
    copy.parity = 20;
    let pack = fs::read_dir(copy_root.join("data"))
        .unwrap()
        .flat_map(|d| fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().path())
        .max_by_key(|p| fs::metadata(p).unwrap().len())
        .unwrap();
    damage(&pack, |b| b[100_000..400_000].fill(0));
    assert!(!full(&copy).ok());
    let fixed = repair(&copy);
    assert!(fixed.ok() && fixed.repaired.len() == 1, "{fixed:?}");
    assert!(fs::read(&pack).unwrap() == fs::read(e.biggest_pack()).unwrap());
}

/// A drive with sectors that can't be read: reading them is an error, not wrong bytes,
/// until the file is written again (the drive then puts it somewhere else).
struct BadSectors {
    inner: LocalBackend,
    /// A file (by the end of its path) and the bytes of it that can't be read.
    bad: std::sync::Mutex<Option<(String, std::ops::Range<u64>)>>,
}

impl BadSectors {
    fn hits(&self, path: &str, range: std::ops::Range<u64>) -> bool {
        self.bad.lock().unwrap().as_ref().is_some_and(|(p, bad)| {
            path.ends_with(p) && range.start < bad.end && bad.start < range.end
        })
    }
}

impl Backend for BadSectors {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if self.hits(path, 0..u64::MAX) {
            return Err(std::io::Error::from_raw_os_error(5)); // EIO
        }
        self.inner.read(path)
    }
    fn read_range(&self, path: &str, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if self.hits(path, offset..offset + len) {
            return Err(std::io::Error::from_raw_os_error(5));
        }
        self.inner.read_range(path, offset, len)
    }
    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        let mut bad = self.bad.lock().unwrap();
        if bad.as_ref().is_some_and(|(p, _)| path.ends_with(p)) {
            *bad = None;
        }
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
    fn lock(&self, exclusive: bool) -> std::io::Result<Option<LockGuard>> {
        self.inner.lock(exclusive)
    }
    fn lock_holders(&self) -> std::io::Result<Vec<LockInfo>> {
        self.inner.lock_holders()
    }
    fn clear_locks(&self, force: bool) -> std::io::Result<Vec<LockInfo>> {
        self.inner.clear_locks(force)
    }
}

#[test]
fn sectors_that_cannot_be_read_are_rebuilt() {
    let e = env();
    fs::write(e.src.join("a"), noise(6 << 20, 1)).unwrap();
    let repo = e.init(Some(PW), 5);
    let snap = e.snap(&repo);
    let before = e.read_all(&repo, &snap);
    let pack = e.biggest_pack();
    let name = pack.file_name().unwrap().to_string_lossy().into_owned();
    let original = fs::read(&pack).unwrap();
    drop(repo);

    // Three sectors in the middle of the pack give read errors.
    let mut repo = VaultRepo::open(
        Box::new(BadSectors {
            inner: LocalBackend::new(&e.root),
            bad: std::sync::Mutex::new(Some((name.clone(), 2_000_000..2_012_288))),
        }),
        Some(PW),
    )
    .unwrap();
    repo.parity = 5;
    let found = full(&repo);
    assert_eq!((found.errors.len(), found.repairable), (1, 1), "{found:?}");
    let fixed = repair(&repo);
    assert!(fixed.ok(), "{:?}", fixed.errors);
    // The unreadable range straddles four 4 KiB blocks.
    assert!(
        fixed.repaired[0].contains("repaired (4 blocks)"),
        "{:?}",
        fixed.repaired
    );
    assert!(fs::read(&pack).unwrap() == original);
    assert!(e.read_all(&repo, &snap) == before);
    assert!(full(&repo).ok());
}

/// Two snapshots sharing one pack, the first holding files the second doesn't: deleting
/// the first leaves that pack mostly unused, so it is repacked.
fn vault_with_a_pack_to_repack(e: &Env, parity: u8) -> (VaultRepo, String, String) {
    for i in 0..6 {
        fs::write(e.src.join(format!("keep{i}")), noise(600 << 10, i + 1)).unwrap();
        fs::write(e.src.join(format!("drop{i}")), noise(600 << 10, i + 100)).unwrap();
    }
    let repo = e.init(Some(PW), parity);
    let first = e.snap(&repo);
    for i in 0..6 {
        fs::remove_file(e.src.join(format!("drop{i}"))).unwrap();
    }
    let second = e.snap(&repo);
    (repo, first, second)
}

/// A pack that bit rot reached is repaired before a prune repacks it. Repacked as it
/// was, its damaged blobs went into a new pack under a new name, with new repair data
/// made from the damage, and the repair data that could have put them right was deleted.
#[test]
fn a_damaged_pack_is_repaired_before_it_is_repacked() {
    let e = env();
    let (repo, first, second) = vault_with_a_pack_to_repack(&e, 10);
    let before = e.read_all(&repo, &second);
    let victim = e.biggest_pack();
    let len = fs::metadata(&victim).unwrap().len() as usize;
    damage(&victim, |b| {
        for at in [len / 10, len / 2, len * 85 / 100] {
            b[at] ^= 0xff;
        }
    });
    assert_eq!(full(&repo).repairable, 1);

    let gc = gc::delete(&repo, &[first.as_str()], &silent).unwrap();
    assert_eq!((gc.packs_repaired, gc.packs_damaged), (1, 0), "{gc:?}");
    assert!(gc.packs_repacked >= 1, "{gc:?}");
    assert!(!victim.exists(), "repacked");
    let after = full(&repo);
    assert!(after.ok(), "{:?}", after.errors);
    assert!(e.read_all(&repo, &second) == before);
}

/// A damaged pack that can't be rebuilt (here: no repair data) is not repacked: it stays
/// under its name, in the index, so a check names it and whatever can still be read from
/// it can be. The rest of the prune goes on.
#[test]
fn a_damaged_pack_beyond_repair_is_left_as_it_is() {
    let e = env();
    let (repo, first, second) = vault_with_a_pack_to_repack(&e, 0);
    let victim = e.biggest_pack();
    let len = fs::metadata(&victim).unwrap().len() as usize;
    damage(&victim, |b| b[len / 2] ^= 0xff);
    let damaged = fs::read(&victim).unwrap();

    let gc = gc::delete(&repo, &[first.as_str()], &silent).unwrap();
    assert_eq!((gc.packs_repaired, gc.packs_damaged), (0, 1), "{gc:?}");
    assert!(fs::read(&victim).unwrap() == damaged, "untouched");
    assert!(
        repo.read_record(&first).is_err(),
        "the snapshot is deleted all the same"
    );
    assert_eq!(repo.read_record(&second).unwrap().meta.id, second);
    let name = victim.file_name().unwrap().to_string_lossy().into_owned();
    let found = full(&repo);
    assert!(
        found.errors.iter().any(|m| m.contains(&name)),
        "the check names the pack that was damaged: {:?}",
        found.errors
    );
}

fn copy_to(repo: &VaultRepo, root: &Path) -> flux_vault::replicate::VaultReplica {
    flux_vault::replicate::replicate(repo.backend(), &LocalBackend::new(root), &Progress::new())
        .unwrap()
}

fn open_at(root: &Path) -> VaultRepo {
    VaultRepo::open(Box::new(LocalBackend::new(root)), Some(PW)).unwrap()
}

/// A pack that goes missing from the Vault without a prune (a damaged filesystem, a
/// folder deleted by hand) is not removed from the copy: the Vault's index still counts
/// on it, and the copy's may be the last one there is. What a prune removes still goes.
#[test]
fn a_copy_keeps_the_packs_the_vault_lost() {
    let e = env();
    let (repo, first, second) = vault_with_a_pack_to_repack(&e, 5);
    let copy_root = e.tmp.path().join("usb/flux-vault");
    copy_to(&repo, &copy_root);
    let victim = e.biggest_pack();
    let in_copy = copy_root.join(victim.strip_prefix(&e.root).unwrap());
    let par_in_copy = copy_root
        .join("parity")
        .join(victim.strip_prefix(&e.root).unwrap());
    assert!(in_copy.is_file() && par_in_copy.is_file());

    fs::remove_file(&victim).unwrap();
    fs::remove_file(parity_of(&e, &victim)).unwrap();
    let r = copy_to(&repo, &copy_root);
    assert_eq!((r.kept_packs, r.removed_files), (1, 0), "{r:?}");
    assert!(in_copy.is_file() && par_in_copy.is_file());
    let copy = open_at(&copy_root);
    assert!(full(&copy).ok(), "the copy still restores everything");
    assert_eq!(e.read_all(&copy, &second).len(), 6);

    // Put back, then pruned: the index is replaced, and the copy follows.
    fs::copy(&in_copy, &victim).unwrap();
    gc::delete(&repo, &[first.as_str()], &silent).unwrap();
    assert!(!victim.exists(), "repacked by the prune");
    let r = copy_to(&repo, &copy_root);
    assert!(r.kept_packs == 0 && r.removed_files > 0, "{r:?}");
    assert!(!in_copy.exists());
    assert!(full(&open_at(&copy_root)).ok());
}

/// A damaged pack in the Vault never replaces the copy's own. Where the copy has none, it
/// is copied with its repair data, and can be rebuilt there.
#[test]
fn a_damaged_pack_never_replaces_the_copys() {
    let e = env();
    fs::write(e.src.join("a"), noise(4 << 20, 1)).unwrap();
    let repo = e.init(Some(PW), 5);
    e.snap(&repo);
    let copy_root = e.tmp.path().join("usb/flux-vault");
    copy_to(&repo, &copy_root);
    let victim = e.biggest_pack();
    let in_copy = copy_root.join(victim.strip_prefix(&e.root).unwrap());
    let good = fs::read(&victim).unwrap();

    // Cut short: another size than the copy's, which was copied over before.
    damage(&victim, |b| b.truncate(b.len() - 100));
    let r = copy_to(&repo, &copy_root);
    assert_eq!(r.damaged_packs, 1, "{r:?}");
    assert!(
        fs::read(&in_copy).unwrap() == good,
        "the copy's own is kept"
    );
    assert!(full(&open_at(&copy_root)).ok());

    // A new copy has no pack of its own: it takes the damaged one and its repair data.
    damage(&victim, |b| {
        *b = good.clone();
        b[good.len() / 2] ^= 0x10;
    });
    let second_root = e.tmp.path().join("nas/flux-vault");
    let r = copy_to(&repo, &second_root);
    assert_eq!(r.damaged_packs, 1, "{r:?}");
    let second = open_at(&second_root);
    assert_eq!(full(&second).repairable, 1);
    assert!(repair(&second).ok());
    assert!(fs::read(second_root.join(victim.strip_prefix(&e.root).unwrap())).unwrap() == good);
}

/// A Vault with no repair data at all heals the same way: a damaged pack and a missing
/// one leave the index on a repair, and the next backup stores their data again. Before,
/// every later backup found the data "already stored" and made snapshots that counted on
/// it, for as long as the Vault lived.
#[test]
fn a_vault_heals_from_the_files_that_are_still_there() {
    let e = env();
    for i in 0..5 {
        fs::write(e.src.join(format!("f{i}")), noise(700 << 10, i + 1)).unwrap();
    }
    let repo = e.init(Some(PW), 0);
    let first = e.snap(&repo);
    let before = e.read_all(&repo, &first);
    let victim = e.biggest_pack();
    let len = fs::metadata(&victim).unwrap().len() as usize;
    damage(&victim, |b| b[len / 2] ^= 0x01);

    // Found, and not healed by backing up again: the index still has every blob.
    assert!(!full(&repo).ok());
    for i in 0..5 {
        fs::write(e.src.join(format!("f{i}")), noise(700 << 10, i + 1)).unwrap();
    }
    fs::write(e.src.join("new"), b"something changed").unwrap();
    e.snap(&repo);
    assert!(!full(&repo).ok(), "no backup stores what the index has");

    // Repaired: one blob is gone, the others of the pack were saved.
    let r = repair(&repo);
    assert_eq!(r.repaired.len(), 1, "{:?}", r.repaired);
    assert!(
        r.repaired[0].contains("the other 1 were taken out"),
        "{:?}",
        r.repaired
    );
    assert!(
        !r.ok() && r.errors.iter().all(|m| m.contains("is missing")),
        "{:?}",
        r.errors
    );
    assert!(!victim.exists());
    let third = e.snap(&repo);
    let healed = full(&repo);
    assert!(healed.ok(), "{:?}", healed.errors);
    assert!(e.read_all(&repo, &first) == before);
    assert_eq!(e.read_all(&repo, &third).len(), 6);

    // A pack that is gone altogether.
    fs::remove_file(e.biggest_pack()).unwrap();
    let r = repair(&repo);
    assert!(
        r.repaired.iter().any(|m| m.contains("it is missing")),
        "{:?}",
        r.repaired
    );
    assert!(!r.ok());
    e.snap_or_unchanged(&repo);
    assert!(full(&repo).ok());
    assert!(e.read_all(&repo, &first) == before);
}

/// What the files no longer have stays lost, and the check goes on saying where: the
/// snapshot that had it, by file. Snapshots made since are whole.
#[test]
fn what_is_gone_from_the_files_too_stays_reported() {
    let e = env();
    fs::write(e.src.join("kept"), noise(600 << 10, 1)).unwrap();
    fs::write(e.src.join("deleted"), noise(600 << 10, 2)).unwrap();
    let repo = e.init(Some(PW), 0);
    let first = e.snap(&repo);
    fs::remove_file(e.biggest_pack()).unwrap();
    fs::remove_file(e.src.join("deleted")).unwrap();
    assert!(!repair(&repo).ok());
    let second = e.snap(&repo);
    let r = full(&repo);
    assert!(
        r.errors.len() == 1 && r.errors[0].contains(&first) && r.errors[0].contains("/deleted"),
        "{:?}",
        r.errors
    );
    assert_eq!(e.read_all(&repo, &second).len(), 1);
}

/// An index file that doesn't open, in a Vault without repair data: a repair writes the
/// index again from the packs' own lists, and everything reads as before.
#[test]
fn an_unreadable_index_is_rebuilt_from_the_packs() {
    let e = env();
    fs::write(e.src.join("a"), noise(2 << 20, 1)).unwrap();
    let repo = e.init(Some(PW), 0);
    let first = e.snap(&repo);
    fs::write(e.src.join("b"), noise(2 << 20, 2)).unwrap();
    let second = e.snap(&repo);
    let before = (e.read_all(&repo, &first), e.read_all(&repo, &second));
    let index_files = e.files("index");
    assert_eq!(index_files.len(), 2);
    damage(&index_files[0], |b| {
        let at = b.len() / 2;
        b[at] ^= 0x40;
    });
    repo.invalidate_index();
    assert!(check(&repo, false, &Progress::new(), &silent).is_err());

    let r = repair(&repo);
    assert!(r.ok(), "{:?}", r.errors);
    assert!(
        r.repaired
            .iter()
            .any(|m| m.contains("written again from the")),
        "{:?}",
        r.repaired
    );
    assert_eq!(e.files("index").len(), 1);
    assert!((e.read_all(&repo, &first), e.read_all(&repo, &second)) == before);
    assert!(full(&repo).ok());
    // And a backup after it stores nothing twice.
    let packs = e.files("data").len();
    fs::write(e.src.join("c"), b"small").unwrap();
    e.snap(&repo);
    assert_eq!(
        e.files("data").len(),
        packs + 2,
        "one data pack, one of trees"
    );
}
