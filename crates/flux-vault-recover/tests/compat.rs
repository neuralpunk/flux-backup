//! The recovery tool reads what Flux's own Vault engine writes: Vaults are made by real
//! backups (through the runner), then listed, restored and verified by the tool alone.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flux_core::profile::{Engine, PasswordSource, Profile, Target, VaultOptions};
use flux_core::progress::{Progress, silent};
use flux_core::{Paths, ProfileId, Scope};
use flux_runner::{BackupJob, RunStatus, Trigger, run_backup};

fn tool(args: &[&str], password: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flux-vault-recover"));
    c.args(args).env_remove("FLUX_PASSWORD");
    if let Some(p) = password {
        c.env("FLUX_PASSWORD", p);
    }
    c.output().expect("run flux-vault-recover")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

struct Env {
    _t: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    id: ProfileId,
    profile: Profile,
}

/// A source folder with the awkward cases, and a Vault profile backing it up.
fn env(encrypted: bool) -> Env {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().to_owned();
    let src = root.join("src");
    fs::create_dir_all(src.join("docs/deep/er")).unwrap();
    fs::create_dir_all(root.join("tgt")).unwrap();
    fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    fs::write(src.join("docs/empty"), "").unwrap();
    fs::write(src.join("docs/deep/er/ünïcödé name.txt"), "unicode").unwrap();
    // Several chunks, and compressible and incompressible parts.
    let mut big = Vec::with_capacity(3 << 20);
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for i in 0..(3 << 20) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        big.push(if i % 3 == 0 { b'a' } else { x as u8 });
    }
    fs::write(src.join("big.bin"), &big).unwrap();
    // A sparse file: 2 MiB, data only at 1 MiB.
    let sparse = fs::File::create(src.join("sparse.img")).unwrap();
    sparse.set_len(2 << 20).unwrap();
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = &sparse;
        f.seek(SeekFrom::Start(1 << 20)).unwrap();
        f.write_all(b"in the middle").unwrap();
    }
    fs::set_permissions(src.join("docs/a.txt"), fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink("docs/a.txt", src.join("link")).unwrap();
    let mut profile = Profile::new(
        "Test",
        Target::Local {
            path: root.join("tgt"),
            fs_uuid: None,
        },
        vec![src],
    );
    profile.engine = Engine::Vault;
    profile.options.notify = false;
    profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    let key = root.join("key");
    fs::write(&key, "s3cret\n").unwrap();
    profile.vault = Some(VaultOptions {
        encryption: encrypted,
        password_source: PasswordSource::File { path: key },
        ..Default::default()
    });
    Env {
        paths: Paths::under(root.join("home"), Scope::User),
        _t: t,
        root,
        id: ProfileId::new("t").unwrap(),
        profile,
    }
}

impl Env {
    fn backup(&self) {
        let r = run_backup(
            &BackupJob {
                paths: &self.paths,
                id: &self.id,
                profile: &self.profile,
                name: None,
                dry_run: false,
                trigger: Trigger::Manual,
                prune: false,
                only_if_due: false,
                ask: None,
                shutdown_after: false,
            },
            &Progress::new(),
            &silent,
        );
        assert_eq!(
            r.record.status,
            RunStatus::Completed,
            "{:?}",
            r.record.error
        );
    }

    fn vault(&self) -> String {
        self.root.join("tgt/flux-vault").display().to_string()
    }

    fn src(&self) -> PathBuf {
        self.root.join("src")
    }
}

/// Where a source file ends up under a restore folder (full path, without the leading /).
fn restored(to: &Path, src: &Path, rel: &str) -> PathBuf {
    to.join(src.strip_prefix("/").unwrap()).join(rel)
}

fn same_files(e: &Env, to: &Path) {
    let src = e.src();
    for rel in [
        "docs/a.txt",
        "docs/empty",
        "docs/deep/er/ünïcödé name.txt",
        "big.bin",
        "sparse.img",
    ] {
        let (a, b) = (src.join(rel), restored(to, &src, rel));
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap(), "{rel}");
        let (ma, mb) = (fs::metadata(&a).unwrap(), fs::metadata(&b).unwrap());
        assert_eq!(ma.mode() & 0o7777, mb.mode() & 0o7777, "{rel} mode");
        assert_eq!(ma.mtime(), mb.mtime(), "{rel} mtime");
    }
    assert_eq!(
        fs::read_link(restored(to, &src, "link")).unwrap(),
        PathBuf::from("docs/a.txt")
    );
}

#[test]
fn encrypted_vaults_are_read_like_flux_reads_them() {
    let e = env(true);
    e.backup();
    fs::write(e.src().join("docs/a.txt"), "alpha, changed").unwrap();
    e.backup();
    let v = e.vault();

    // No password: a clear message; a wrong one: refused.
    let o = tool(&[&v, "snapshots"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("give its password"), "{}", err(&o));
    let o = tool(&[&v, "snapshots"], Some("nope"));
    assert!(err(&o).contains("wrong password"), "{}", err(&o));

    // The same snapshots Flux lists.
    let repo = flux_runner::repo::AnyRepo::open(&e.paths, &e.id, &e.profile, None).unwrap();
    let snaps = repo.snapshots().unwrap();
    let o = tool(&[&v, "snapshots"], Some("s3cret"));
    assert!(o.status.success(), "{}", err(&o));
    let listed = out(&o);
    for s in &snaps {
        assert!(listed.contains(&s.id[..16]), "{listed}");
    }
    assert!(listed.contains("(t)"), "{listed}");

    // The files of the latest snapshot, under a folder.
    let docs = e.src().join("docs").display().to_string();
    let o = tool(
        &[&v, "files", "latest", docs.trim_start_matches('/')],
        Some("s3cret"),
    );
    let files = out(&o);
    assert!(
        files.contains("ünïcödé name.txt") && files.contains("docs/a.txt"),
        "{files}"
    );
    assert!(!files.contains("big.bin"), "only under the folder: {files}");

    // A full restore matches the source; sparse files stay sparse.
    let to = e.root.join("restore");
    let o = tool(
        &[&v, "restore", "latest", to.to_str().unwrap()],
        Some("s3cret"),
    );
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("Restored 5 files"), "{}", out(&o));
    same_files(&e, &to);
    let sparse = fs::metadata(restored(&to, &e.src(), "sparse.img")).unwrap();
    assert!(
        sparse.blocks() * 512 < 1 << 20,
        "holes restored: {} blocks",
        sparse.blocks()
    );
    // Never into a folder with something in it.
    let o = tool(
        &[&v, "restore", "latest", to.to_str().unwrap()],
        Some("s3cret"),
    );
    assert!(err(&o).contains("isn't empty"), "{}", err(&o));

    // The first snapshot, by id prefix, and one file of it.
    let first = &snaps[0].id[..8];
    let only = e.root.join("only");
    let a = e.src().join("docs/a.txt").display().to_string();
    let o = tool(
        &[
            &v,
            "restore",
            first,
            only.to_str().unwrap(),
            a.trim_start_matches('/'),
        ],
        Some("s3cret"),
    );
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(
        fs::read_to_string(restored(&only, &e.src(), "docs/a.txt")).unwrap(),
        "alpha"
    );
    assert!(!restored(&only, &e.src(), "big.bin").exists());

    // Verify reads everything back; a damaged pack is found.
    let o = tool(&[&v, "verify"], Some("s3cret"));
    assert!(
        o.status.success() && out(&o).contains("Everything reads back"),
        "{}",
        out(&o)
    );
    damage_largest_pack(&e.root.join("tgt/flux-vault/data"));
    let o = tool(&[&v, "verify"], Some("s3cret"));
    assert_eq!(o.status.code(), Some(3), "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("damaged"), "{}", err(&o));
}

fn damage_largest_pack(data: &Path) {
    let mut packs: Vec<PathBuf> = fs::read_dir(data)
        .unwrap()
        .flatten()
        .flat_map(|d| fs::read_dir(d.path()).unwrap().flatten().map(|f| f.path()))
        .collect();
    packs.sort_by_key(|p| fs::metadata(p).unwrap().len());
    let p = packs.last().unwrap();
    let mut bytes = fs::read(p).unwrap();
    bytes[100] ^= 0xff;
    let mut perm = fs::metadata(p).unwrap().permissions();
    perm.set_mode(0o644);
    fs::set_permissions(p, perm).unwrap();
    fs::write(p, bytes).unwrap();
}

#[test]
fn unencrypted_and_format_1_vaults() {
    let e = env(false);
    // A Vault from before 1.0: format 1 (no holes in trees).
    let vault_dir = e.root.join("tgt/flux-vault");
    {
        let root = flux_runner::repo::vault_root(&e.profile);
        let backend = Box::new(flux_vault::LocalBackend::new(&root));
        flux_vault::VaultRepo::init(backend, None, flux_vault::crypto::KdfParams::TESTING).unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(vault_dir.join("config")).unwrap()).unwrap();
        config["format"] = 1.into();
        fs::write(
            vault_dir.join("config"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
    }
    e.backup();
    // The drive's folder works as well as the Vault's own.
    let drive = e.root.join("tgt").display().to_string();
    let to = e.root.join("restore");
    let o = tool(&[&drive, "restore", "latest", to.to_str().unwrap()], None);
    assert!(o.status.success(), "{}", err(&o));
    same_files(&e, &to);
    let o = tool(&[&drive, "verify", "latest"], None);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(tool(&["--version"], None).status.success());
    assert_eq!(tool(&[&drive], None).status.code(), Some(2));
    assert_eq!(
        tool(&[&drive, "files", "nonexistent"], None).status.code(),
        Some(1)
    );
}

#[test]
fn an_unlock_file_and_a_tampered_record() {
    let mut e = env(true);
    let file = e.root.join("unlock.key");
    flux_runner::repo::create_unlock_file(&file).unwrap();
    e.profile.vault.as_mut().unwrap().unlock_file = Some(file.clone());
    e.backup();
    let v = e.vault();
    let o = tool(&[&v, "snapshots"], Some("s3cret"));
    assert!(err(&o).contains("unlock file"), "{}", err(&o));
    let o = tool(
        &[&v, "--unlock-file", file.to_str().unwrap(), "snapshots"],
        Some("s3cret"),
    );
    assert!(o.status.success(), "{}", err(&o));

    // Someone with the drive swaps a snapshot record for an unencrypted one: refused.
    let snaps = vault_files(&e.root.join("tgt/flux-vault/snapshots"));
    let mut forged = vec![0u8];
    forged.extend_from_slice(br#"{"meta":{"started":"2030-01-01T00:00:00Z"},"tree":"00"}"#);
    let mut perm = fs::metadata(&snaps[0]).unwrap().permissions();
    perm.set_mode(0o644);
    fs::set_permissions(&snaps[0], perm).unwrap();
    fs::write(&snaps[0], forged).unwrap();
    let o = tool(
        &[&v, "--unlock-file", file.to_str().unwrap(), "snapshots"],
        Some("s3cret"),
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("tampered"), "{}", err(&o));
}

/// A snapshot that isn't what Flux wrote (an unencrypted Vault can be rewritten by whoever
/// has the drive) restores a link, then a file "inside" it: nothing may be written where
/// the link points, by either way of saying so in a tree.
#[test]
fn a_forged_snapshot_cannot_write_outside_the_restore_folder() {
    use flux_fs::entry::Kind;
    use flux_vault::format::{BlobKind, IndexFile, Node, Tree};
    use flux_vault::pack::{Packer, encode_blob, write_index};
    let e = env(false);
    e.backup();
    let repo = flux_vault::VaultRepo::open(
        Box::new(flux_vault::LocalBackend::new(e.root.join("tgt/flux-vault"))),
        None,
    )
    .unwrap();
    let outside = e.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let node = |name: &str, kind: Kind, content: Vec<[u8; 32]>, size: u64| Node {
        name: name.as_bytes().to_vec(),
        kind,
        mode: 0o644,
        uid: 0,
        gid: 0,
        size,
        mtime: 0,
        mtime_nsec: 0,
        dev: 0,
        ino: 0,
        nlink: 1,
        rdev: 0,
        link: (kind == Kind::Symlink).then(|| outside.to_str().unwrap().as_bytes().to_vec()),
        hash: None,
        xattrs: Vec::new(),
        content,
        subtree: None,
        holes: Vec::new(),
    };
    let payload = b"planted by the snapshot\n";
    let packer = Packer::new(repo.backend(), &repo.crypto);
    let (data, sealed) = encode_blob(&repo.crypto, payload, 0).unwrap();
    packer
        .add(BlobKind::Data, data, payload.len(), &sealed)
        .unwrap();
    let add_tree = |tree: Tree| {
        let raw = tree.encode(2).unwrap();
        let (id, sealed) = encode_blob(&repo.crypto, &raw, 0).unwrap();
        packer.add(BlobKind::Tree, id, raw.len(), &sealed).unwrap();
        id
    };
    let file = |name: &str| node(name, Kind::File, vec![data], payload.len() as u64);
    // A subtree hung on a link …
    let below = add_tree(Tree {
        nodes: vec![file("under-the-link")],
    });
    let mut hung = node("hung", Kind::Symlink, Vec::new(), 0);
    hung.subtree = Some(below);
    // … and a name with the link as its folder.
    let root = add_tree(Tree {
        nodes: vec![
            hung,
            node("lnk", Kind::Symlink, Vec::new(), 0),
            file("lnk/planted"),
            file("ok.txt"),
        ],
    });
    let packs = packer.finish().unwrap();
    write_index(
        repo.backend(),
        &repo.crypto,
        &IndexFile {
            packs,
            supersedes: Vec::new(),
        },
    )
    .unwrap();
    let id = repo.records().unwrap().pop().unwrap().meta.id;
    let mut rec = repo.read_record(&id).unwrap();
    rec.tree = flux_vault::crypto::hex_id(&root);
    rec.tree_format = 2;
    repo.write_record(&rec).unwrap();

    let to = e.root.join("restored");
    let o = tool(
        &[&e.vault(), "restore", "latest", to.to_str().unwrap()],
        None,
    );
    assert!(
        fs::read_dir(&outside).unwrap().next().is_none(),
        "written outside the restore folder: {}{}",
        out(&o),
        err(&o)
    );
    assert_eq!(fs::read(to.join("ok.txt")).unwrap(), payload);
    assert!(out(&o).contains("lnk/planted") || err(&o).contains("lnk/planted"));
}

fn vault_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .collect()
}
