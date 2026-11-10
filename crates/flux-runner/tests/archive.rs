//! Snapshot export, checked with the standard tools people would open the archives with
//! (GNU tar, unzip, Python's readers, sha256sum). A check whose tool isn't installed is
//! skipped.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use flux_core::profile::{Engine, PasswordSource, Profile, Target, VaultOptions};
use flux_core::progress::{Progress, silent};
use flux_core::{Paths, ProfileId, Scope};
use flux_runner::archive::{self, Checksums, ExportRequest, Format};
use flux_runner::repo::AnyRepo;
use flux_runner::{BackupJob, RunStatus, Trigger, run_backup};

fn have(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run(cmd: &str, dir: &Path) -> (bool, String) {
    let o = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(dir)
        .output()
        .unwrap();
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

struct Env {
    _t: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    id: ProfileId,
    profile: Profile,
    xattr: bool,
    acl: bool,
}

fn env(engine: Engine) -> Env {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().to_owned();
    let src = root.join("src");
    fs::create_dir_all(src.join("docs/sub")).unwrap();
    fs::create_dir_all(root.join("tgt")).unwrap();
    fs::create_dir_all(root.join("out")).unwrap();
    fs::write(src.join("docs/a.txt"), "alpha").unwrap();
    fs::write(src.join("docs/sub/ünï cödé.txt"), "unicode").unwrap();
    fs::write(src.join("docs/empty"), "").unwrap();
    fs::write(
        src.join("big.bin"),
        (0..300_000u32)
            .flat_map(|i| i.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::set_permissions(src.join("docs/a.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("a.txt", src.join("docs/link")).unwrap();
    fs::hard_link(src.join("docs/a.txt"), src.join("docs/hard")).unwrap();
    let xattr = have("setfattr")
        && run(
            &format!(
                "setfattr -n user.flux -v kept '{}'",
                src.join("docs/a.txt").display()
            ),
            &root,
        )
        .0;
    let acl = have("setfacl")
        && run(
            &format!(
                "setfacl -m u:65534:r '{}'",
                src.join("docs/empty").display()
            ),
            &root,
        )
        .0;
    let mut profile = Profile::new(
        "Test",
        Target::Local {
            path: root.join("tgt"),
            fs_uuid: None,
        },
        vec![src],
    );
    profile.engine = engine;
    profile.options.notify = false;
    profile.unusual_changes = flux_core::profile::Sensitivity::Off;
    if engine == Engine::Vault {
        fs::write(root.join("key"), "pw\n").unwrap();
        profile.vault = Some(VaultOptions {
            password_source: PasswordSource::File {
                path: root.join("key"),
            },
            ..Default::default()
        });
    }
    let e = Env {
        paths: Paths::under(root.join("home"), Scope::User),
        _t: t,
        root,
        id: ProfileId::new("t").unwrap(),
        profile,
        xattr,
        acl,
    };
    let r = run_backup(
        &BackupJob {
            paths: &e.paths,
            id: &e.id,
            profile: &e.profile,
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
    e
}

impl Env {
    fn export(
        &self,
        paths: Vec<PathBuf>,
        format: Format,
        out: &str,
        sums: Option<Checksums>,
    ) -> Result<archive::ExportOutcome, String> {
        let repo = AnyRepo::open(&self.paths, &self.id, &self.profile, None).unwrap();
        let snap = repo.find("latest").unwrap();
        archive::export(
            &repo,
            &ExportRequest {
                snapshot: &snap,
                paths,
                format,
                out: self.root.join("out").join(out),
                checksums: sums,
            },
            &Progress::new(),
            &silent,
        )
    }

    fn src(&self) -> PathBuf {
        self.root.join("src")
    }

    /// The source path as archive members name it (no leading /).
    fn rel(&self, p: &str) -> String {
        format!("{}/{p}", self.src().display())
            .trim_start_matches('/')
            .to_owned()
    }
}

fn check_tar(e: &Env) {
    let out = e
        .export(
            Vec::new(),
            Format::TarZst,
            "all.tar.zst",
            Some(Checksums::Sha256),
        )
        .unwrap();
    assert_eq!(
        out.files, 4,
        "a.txt (with its hard link once), unicode, empty, big: {out:?}"
    );
    assert!(out.left_out.is_empty(), "{:?}", out.left_out);
    assert!(out.size > 0 && out.manifest.as_ref().unwrap().is_file());
    // Nothing left behind next to the archive but it and its manifest.
    let names: Vec<_> = fs::read_dir(e.root.join("out"))
        .unwrap()
        .flatten()
        .map(|f| f.file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    if !(have("tar") && have("zstd")) {
        return;
    }
    let x = e.root.join("x");
    fs::create_dir_all(&x).unwrap();
    let (ok, msg) = run(
        &format!(
            "tar --zstd --xattrs --xattrs-include='*' --acls -xf '{}'",
            out.archive.display()
        ),
        &x,
    );
    assert!(ok, "{msg}");
    let got = |p: &str| x.join(e.rel(p));
    for p in [
        "docs/a.txt",
        "docs/sub/ünï cödé.txt",
        "docs/empty",
        "big.bin",
    ] {
        assert_eq!(
            fs::read(e.src().join(p)).unwrap(),
            fs::read(got(p)).unwrap(),
            "{p}"
        );
        let (a, b) = (
            fs::metadata(e.src().join(p)).unwrap(),
            fs::metadata(got(p)).unwrap(),
        );
        assert_eq!(a.mtime(), b.mtime(), "{p}");
        assert_eq!(a.mtime_nsec(), b.mtime_nsec(), "{p} to the nanosecond");
    }
    assert_eq!(
        fs::metadata(got("docs/a.txt")).unwrap().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::read_link(got("docs/link")).unwrap(),
        PathBuf::from("a.txt")
    );
    assert_eq!(
        fs::metadata(got("docs/hard")).unwrap().ino(),
        fs::metadata(got("docs/a.txt")).unwrap().ino(),
        "the hard link is a hard link again"
    );
    if e.xattr && have("getfattr") {
        let (ok, v) = run(
            &format!(
                "getfattr --only-values -n user.flux '{}' 2>/dev/null",
                got("docs/a.txt").display()
            ),
            &x,
        );
        assert!(ok && v == "kept", "{v}");
    }
    if e.acl && have("getfacl") {
        let (ok, v) = run(
            &format!("getfacl -n '{}' 2>/dev/null", got("docs/empty").display()),
            &x,
        );
        assert!(ok && v.contains("user:65534:r--"), "the ACL came back: {v}");
    }
    // The manifest checks out where the archive was unpacked.
    let (ok, msg) = run(
        &format!("sha256sum -c --quiet '{}'", out.manifest.unwrap().display()),
        &x,
    );
    assert!(ok, "{msg}");
    // Owners are recorded (as numbers) and readable by Python's tarfile too.
    if have("python3") {
        let (ok, listing) = run(
            &format!(
                "zstd -dc '{}' | python3 -c \"import tarfile,sys; t=tarfile.open(fileobj=sys.stdin.buffer, mode='r|'); [print(m.name, m.uid, m.pax_headers.get('mtime','')) for m in t]\"",
                out.archive.display()
            ),
            &x,
        );
        assert!(ok, "{listing}");
        let uid = fs::metadata(e.src().join("docs/a.txt")).unwrap().uid();
        let line = listing
            .lines()
            .find(|l| l.starts_with(&e.rel("docs/a.txt")))
            .unwrap();
        assert!(line.contains(&format!(" {uid} ")), "{line}");
    }
}

#[test]
fn mirror_snapshot_as_tar_zst() {
    check_tar(&env(Engine::Mirror));
}

#[test]
fn vault_snapshot_as_tar_zst() {
    check_tar(&env(Engine::Vault));
}

#[test]
fn a_folder_as_zip_with_blake3_sums() {
    let e = env(Engine::Mirror);
    let docs = e.src().join("docs");
    let out = e
        .export(
            vec![docs.clone()],
            Format::Zip,
            "docs.zip",
            Some(Checksums::Blake3),
        )
        .unwrap();
    assert_eq!(
        out.files, 4,
        "a.txt, its hard link as a copy, unicode, empty: {out:?}"
    );
    assert!(
        out.left_out.iter().any(|l| l == "1 symbolic link left out"),
        "{:?}",
        out.left_out
    );
    assert!(
        out.left_out.iter().any(|l| l.contains("hard link")),
        "{:?}",
        out.left_out
    );
    // The manifest: BLAKE3 of each file, named as in the archive.
    let manifest = fs::read_to_string(out.manifest.as_ref().unwrap()).unwrap();
    let line = format!(
        "{}  {}\n",
        blake3::hash(b"alpha").to_hex(),
        e.rel("docs/a.txt")
    );
    assert!(manifest.contains(&line), "{manifest}");
    assert!(!manifest.contains("big.bin"), "only the folder");
    if have("unzip") {
        let x = e.root.join("z");
        fs::create_dir_all(&x).unwrap();
        let (ok, msg) = run(&format!("unzip -q '{}'", out.archive.display()), &x);
        assert!(ok, "{msg}");
        let got = x.join(e.rel("docs/sub/ünï cödé.txt"));
        assert_eq!(fs::read_to_string(got).unwrap(), "unicode");
        assert_eq!(
            fs::read_to_string(x.join(e.rel("docs/hard"))).unwrap(),
            "alpha"
        );
        assert_eq!(
            fs::metadata(x.join(e.rel("docs/a.txt"))).unwrap().mode() & 0o777,
            0o640
        );
        let (a, b) = (
            fs::metadata(docs.join("a.txt")).unwrap().mtime(),
            fs::metadata(x.join(e.rel("docs/a.txt"))).unwrap().mtime(),
        );
        assert_eq!(a, b, "times to the second");
    }
    if have("python3") {
        let (ok, msg) = run(
            &format!(
                "python3 -c \"import zipfile; z=zipfile.ZipFile('{}'); assert z.testzip() is None; print(len(z.namelist()))\"",
                out.archive.display()
            ),
            &e.root,
        );
        assert!(ok, "{msg}");
    }
}

#[test]
fn what_export_refuses() {
    let e = env(Engine::Mirror);
    let missing = e.src().join("nope");
    let err = e
        .export(vec![missing], Format::Zip, "x.zip", None)
        .unwrap_err();
    assert!(err.contains("isn't in snapshot"), "{err}");
    fs::write(e.root.join("out/taken.zip"), "").unwrap();
    let err = e
        .export(Vec::new(), Format::Zip, "taken.zip", None)
        .unwrap_err();
    assert!(err.contains("exists already"), "{err}");
    let names: Vec<_> = fs::read_dir(e.root.join("out"))
        .unwrap()
        .flatten()
        .map(|f| f.file_name())
        .collect();
    assert_eq!(
        names.len(),
        1,
        "no staging or partial files left: {names:?}"
    );
}

/// The archive is made new in the chosen folder: a link someone left under the name of
/// the unfinished archive (or of the checksum file's temporary one) is replaced, never
/// written through. As root, in a folder of someone else's, that would overwrite any file.
#[test]
fn export_never_writes_through_a_planted_link() {
    let e = env(Engine::Mirror);
    let victim = e.root.join("victim");
    fs::write(&victim, "precious").unwrap();
    let out = e.root.join("out");
    std::os::unix::fs::symlink(&victim, out.join(".x.zip.partial")).unwrap();
    std::os::unix::fs::symlink(
        &victim,
        out.join(format!(".x.zip.sha256.tmp-{}", std::process::id())),
    )
    .unwrap();
    let o = e
        .export(Vec::new(), Format::Zip, "x.zip", Some(Checksums::Sha256))
        .unwrap();
    assert!(o.size > 0);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    assert!(!out.join("x.zip").is_symlink());
    let mut names: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["x.zip", "x.zip.sha256"]);
}
