//! What the Vault engine does when the storage or a source file misbehaves in the middle of
//! a backup: a wrapper around the local backend fails (or acts) at chosen moments.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use flux_core::profile::{Profile, Target, VaultOptions};
use flux_core::progress::{Progress, silent};
use flux_fs::outcome::BackupOutcome;
use flux_fs::snapshot::SnapshotStatus;
use flux_transport::lock::LockInfo;
use flux_vault::backend::{Backend, LockGuard};
use flux_vault::backup::{BackupRequest, backup};
use flux_vault::crypto::KdfParams;
use flux_vault::{LocalBackend, Owner, VaultError, VaultRepo, check};

const PW: &str = "correct horse battery staple";

type Hook = Box<dyn Fn() + Send + Sync>;

struct Faults {
    /// Fail the `write_pack` call at which this countdown reaches 0 (negative: never).
    fail_pack_in: AtomicI64,
    /// Run once, at the first `write_pack`.
    at_first_pack: Mutex<Option<Hook>>,
    /// Armed: the place goes away after `config` is next read (a drive pulled out): every
    /// folder then lists as empty and nothing can be read.
    vanish: AtomicI64,
}

const ARMED: i64 = 1;
const GONE: i64 = 2;

struct Faulty {
    inner: LocalBackend,
    f: Arc<Faults>,
}

impl Backend for Faulty {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        if self.f.vanish.load(SeqCst) == GONE {
            return Err(io::ErrorKind::NotFound.into());
        }
        let r = self.inner.read(path);
        if path == "config" {
            let _ = self.f.vanish.compare_exchange(ARMED, GONE, SeqCst, SeqCst);
        }
        r
    }
    fn read_range(&self, path: &str, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        self.inner.read_range(path, offset, len)
    }
    fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.inner.write(path, data)
    }
    fn write_pack(&self, path: &str, data: &[u8]) -> io::Result<()> {
        if let Some(hook) = self.f.at_first_pack.lock().unwrap().take() {
            hook();
        }
        if self.f.fail_pack_in.fetch_sub(1, SeqCst) == 0 {
            return Err(io::Error::from_raw_os_error(28)); // ENOSPC, once
        }
        self.inner.write_pack(path, data)
    }
    fn flush_packs(&self) -> io::Result<()> {
        self.inner.flush_packs()
    }
    fn list(&self, dir: &str) -> io::Result<Vec<(String, u64)>> {
        if self.f.vanish.load(SeqCst) == GONE {
            return Ok(Vec::new()); // a missing folder is an empty one
        }
        self.inner.list(dir)
    }
    fn remove(&self, path: &str) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn exists(&self, path: &str) -> io::Result<bool> {
        self.inner.exists(path)
    }
    fn lock(&self, exclusive: bool) -> io::Result<Option<LockGuard>> {
        self.inner.lock(exclusive)
    }
    fn lock_holders(&self) -> io::Result<Vec<LockInfo>> {
        self.inner.lock_holders()
    }
    fn clear_locks(&self, force: bool) -> io::Result<Vec<LockInfo>> {
        self.inner.clear_locks(force)
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    root: PathBuf,
    profile: Profile,
    owner: Owner,
    faults: Arc<Faults>,
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
        root: flux_vault::local_root(&target),
        _tmp: tmp,
        src,
        profile,
        owner: Owner {
            profile: "t".into(),
            host: "h".into(),
            user: "u".into(),
        },
        faults: Arc::new(Faults {
            fail_pack_in: AtomicI64::new(-1),
            at_first_pack: Mutex::new(None),
            vanish: AtomicI64::new(0),
        }),
    }
}

impl Env {
    fn init(&self) -> VaultRepo {
        VaultRepo::init(
            Box::new(Faulty {
                inner: LocalBackend::new(&self.root),
                f: self.faults.clone(),
            }),
            Some(PW),
            KdfParams::TESTING,
        )
        .unwrap()
    }
    fn backup(&self, repo: &VaultRepo) -> Result<BackupOutcome, VaultError> {
        backup(
            repo,
            &BackupRequest {
                owner: &self.owner,
                profile: &self.profile,
                name: None,
                dry_run: false,
                never: vec![self.root.clone()],
                cache_dir: None,
                keep: Vec::new(),
            },
            &Progress::new(),
            &silent,
        )
    }
}

/// Bytes that don't compress or deduplicate.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(len);
    v
}

/// A pack that can't be written (the drive is full, the connection dropped) holds the
/// data of many files, not only of the one that filled it. The backup must not make a
/// snapshot that names one failed file and silently lacks the data of the others.
#[test]
fn a_failed_pack_write_makes_no_snapshot() {
    let e = env();
    for i in 0..60u64 {
        fs::write(e.src.join(format!("f{i:02}")), noise(i + 1, 1 << 20)).unwrap();
    }
    let repo = e.init();
    e.faults.fail_pack_in.store(1, SeqCst); // the second pack written fails, once
    let err = e.backup(&repo).unwrap_err();
    assert!(matches!(err, VaultError::Store(_)), "{err}");
    assert!(
        repo.snapshots(&e.owner).unwrap().is_empty(),
        "no snapshot of a backup that lost data"
    );

    // The next run stores everything, and all of it is there.
    let snap = match e.backup(&repo).unwrap() {
        BackupOutcome::Created { snapshot, .. } => snapshot,
        o => panic!("{o:?}"),
    };
    assert_eq!(snap.status, SnapshotStatus::Complete);
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{report:?}");
}

/// A sparse file cut short while it's being read: its holes were found before the read
/// and would end past the size that was stored, which makes the whole folder's tree
/// unreadable (and with it every restore of the snapshot). The file is left out and
/// listed; everything else is restorable, and so is the next backup.
#[test]
fn a_sparse_file_cut_short_while_read_does_not_spoil_the_snapshot() {
    use std::os::unix::fs::FileExt;
    let e = env();
    fs::write(e.src.join("other.txt"), b"an innocent bystander").unwrap();
    let path = e.src.join("disk.img");
    {
        // 40 MiB of data (more than one pack), then a hole to 1 GiB.
        let f = fs::File::create(&path).unwrap();
        let block = noise(3, 1 << 20);
        for i in 0..40u64 {
            f.write_all_at(&block[..], i << 20).unwrap();
            // Each block differs, so nothing deduplicates and packs fill.
            f.write_all_at(&i.to_le_bytes(), i << 20).unwrap();
        }
        f.set_len(1 << 30).unwrap();
        f.sync_all().unwrap();
    }
    let repo = e.init();
    // The first pack is written while the image is being read: cut the file short then.
    let cut = path.clone();
    *e.faults.at_first_pack.lock().unwrap() = Some(Box::new(move || {
        fs::OpenOptions::new()
            .write(true)
            .open(&cut)
            .unwrap()
            .set_len(1 << 20)
            .unwrap();
    }));
    let snap = match e.backup(&repo).unwrap() {
        BackupOutcome::Created { snapshot, .. } => snapshot,
        o => panic!("{o:?}"),
    };
    // Whatever the filesystem does with holes, the snapshot can be read back in full.
    let record = repo.read_record(&snap.id).unwrap();
    let flat = repo.flatten(&record).expect("the snapshot's trees decode");
    assert!(flat.iter().any(|(e, _)| e.path.ends_with(b"other.txt")));
    let report = check::check(&repo, true, &Progress::new(), &silent).unwrap();
    assert!(report.ok(), "{report:?}");
    // And the next backup takes the file as it now is.
    let next = match e.backup(&repo).unwrap() {
        BackupOutcome::Created { snapshot, .. } => snapshot,
        o => panic!("{o:?}"),
    };
    assert_eq!(next.status, SnapshotStatus::Complete, "{:?}", next.errors);
    let record = repo.read_record(&next.id).unwrap();
    let flat = repo.flatten(&record).unwrap();
    let img = flat
        .iter()
        .find(|(e, _)| e.path.ends_with(b"disk.img"))
        .expect("the image is in the next snapshot");
    assert_eq!(img.0.size, 1 << 20);
}

/// Another computer takes this backup's lock for abandoned and prunes the Vault while the
/// backup runs: the packs written so far are in no index yet and are deleted as left
/// over. The backup must notice before it writes a snapshot naming them.
#[test]
fn a_backup_pruned_underneath_makes_no_snapshot() {
    use std::sync::atomic::AtomicBool;
    let e = env();
    for i in 0..40u64 {
        fs::write(e.src.join(format!("f{i:02}")), noise(i + 100, 1 << 20)).unwrap();
    }
    let repo = e.init();
    // Stands in for the other computer's prune: removes the first pack that appears.
    let data = e.root.join("data");
    let done = Arc::new(AtomicBool::new(false));
    let remover = {
        let done = done.clone();
        std::thread::spawn(move || {
            while !done.load(SeqCst) {
                for sub in fs::read_dir(&data).into_iter().flatten().flatten() {
                    for f in fs::read_dir(sub.path()).into_iter().flatten().flatten() {
                        if !f.file_name().to_string_lossy().starts_with('.')
                            && fs::remove_file(f.path()).is_ok()
                        {
                            done.store(true, SeqCst);
                        }
                    }
                }
                std::thread::yield_now();
            }
        })
    };
    let r = e.backup(&repo);
    done.store(true, SeqCst);
    remover.join().unwrap();
    let err = r.unwrap_err();
    assert!(matches!(err, VaultError::Store(_)), "{err}");
    assert!(repo.snapshots(&e.owner).unwrap().is_empty());
}

/// A Vault being copied goes away mid-copy (its drive is pulled out). Its folders then
/// list as empty, which must not be read as "everything was deleted": the copy keeps what
/// it has.
#[test]
fn a_source_that_vanishes_does_not_empty_its_copy() {
    let e = env();
    fs::write(e.src.join("a.txt"), noise(7, 1 << 20)).unwrap();
    let repo = e.init();
    e.backup(&repo).unwrap();
    let copy_root = e._tmp.path().join("copy");
    let replica = LocalBackend::new(&copy_root);
    flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new()).unwrap();
    let files = |dir: &str| fs::read_dir(copy_root.join(dir)).unwrap().count();
    let before = (files("snapshots"), files("index"), files("keys"));
    assert!(
        before.0 == 1 && before.1 >= 1 && before.2 >= 1,
        "{before:?}"
    );

    e.faults.vanish.store(ARMED, SeqCst);
    let r = flux_vault::replicate::replicate(repo.backend(), &replica, &Progress::new());
    assert!(r.is_err(), "{r:?}");
    assert_eq!((files("snapshots"), files("index"), files("keys")), before);
}
