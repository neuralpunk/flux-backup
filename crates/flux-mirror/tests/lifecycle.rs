//! Retention, deletion, diff and history on a real Mirror repository.

use std::fs;
use std::path::{Path, PathBuf};

use flux_core::profile::{Profile, Retention, Target};
use flux_core::progress::{Progress, silent};
use flux_core::{HumanDuration, ProfileId};
use flux_fs::diff::{ChangeKind, diff};
use flux_fs::history;
use flux_mirror::manifest::{Entry, ManifestError, ManifestReader};
use flux_mirror::prune::prune_locked;
use flux_mirror::{BackupOutcome, BackupRequest, backup, entries_of, open_repo, scan_live};

struct Env {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    id: ProfileId,
    profile: Profile,
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
            path: target,
            fs_uuid: None,
        },
        vec![src.clone()],
    );
    profile.options.snapshot_if_unchanged = true;
    Env {
        _tmp: tmp,
        src,
        id: ProfileId::new("t").unwrap(),
        profile,
    }
}

impl Env {
    fn snap(&self, prune: bool) -> (String, Vec<String>) {
        let out = backup(
            &BackupRequest {
                id: &self.id,
                profile: &self.profile,
                name: None,
                dry_run: false,
                prune,
            },
            &Progress::new(),
            &silent,
        )
        .unwrap();
        match out {
            BackupOutcome::Created { snapshot, pruned } => (snapshot.id, pruned),
            o => panic!("{o:?}"),
        }
    }
}

fn keep_last(n: u32) -> Retention {
    Retention {
        enabled: true,
        keep_last: n,
        keep_within: None,
        keep_hourly: 0,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
        keep_yearly: 0,
        max_age: None,
        keep_named: true,
        min_free_space: None,
        min_free_inodes_percent: None,
    }
}

#[test]
fn prune_pin_and_delete() {
    let mut e = env();
    fs::write(e.src.join("f"), "1").unwrap();
    let ids: Vec<String> = (0..5).map(|_| e.snap(false).0).collect();
    let repo = open_repo(&e.id, &e.profile).unwrap();
    {
        let _l = repo.lock().unwrap();
        repo.update_meta(&ids[0], |m| m.pinned = true).unwrap();
        repo.update_meta(&ids[1], |m| m.name = Some("keepme".into()))
            .unwrap();
    }

    // Dry run removes nothing.
    let policy = keep_last(2);
    {
        let _l = repo.lock().unwrap();
        let r = prune_locked(&repo, &policy, true, &silent).unwrap();
        assert_eq!(r.removed, [ids[2].clone()]);
        assert_eq!(repo.snapshots().unwrap().len(), 5);
    }

    // Auto-prune after a backup: keeps pinned, named, and the last two.
    e.profile.retention = policy;
    let (newest, pruned) = e.snap(true);
    let left: Vec<String> = repo
        .snapshots()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        left,
        [
            ids[0].clone(),
            ids[1].clone(),
            ids[4].clone(),
            newest.clone()
        ]
    );
    assert_eq!(pruned, [ids[2].clone(), ids[3].clone()]);
    assert!(
        !repo
            .snapshots_dir()
            .join(format!(".deleting-{}", ids[2]))
            .exists()
    );

    // Deleting the newest re-points `latest`.
    {
        let _l = repo.lock().unwrap();
        repo.delete_snapshot(&newest).unwrap();
    }
    assert_eq!(
        fs::read_link(repo.root().join("latest")).unwrap(),
        Path::new("snapshots").join(&ids[4])
    );
    // The remaining snapshots are intact.
    assert!(
        repo.data_dir(&ids[4])
            .join(e.src.strip_prefix("/").unwrap())
            .join("f")
            .exists()
    );
}

#[test]
fn disabled_retention_keeps_everything() {
    let mut e = env();
    fs::write(e.src.join("f"), "1").unwrap();
    let mut r = keep_last(1);
    r.enabled = false;
    r.max_age = Some(HumanDuration::from_secs(1));
    e.profile.retention = r;
    for _ in 0..3 {
        e.snap(true);
    }
    assert_eq!(
        open_repo(&e.id, &e.profile)
            .unwrap()
            .snapshots()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn diff_snapshots_and_live() {
    let e = env();
    fs::create_dir_all(e.src.join("d")).unwrap();
    fs::write(e.src.join("d/keep"), "same").unwrap();
    fs::write(e.src.join("d/edit"), "v1").unwrap();
    fs::write(e.src.join("d/gone"), "x").unwrap();
    let (a, _) = e.snap(false);
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(e.src.join("d/edit"), "version 2").unwrap();
    fs::remove_file(e.src.join("d/gone")).unwrap();
    fs::write(e.src.join("d/new"), "n").unwrap();
    let (b, _) = e.snap(false);

    let repo = open_repo(&e.id, &e.profile).unwrap();
    let m = |id: &str| ManifestReader::open(&repo.manifest_path(id)).unwrap();
    let (changes, sum) = diff(m(&a), m(&b), b"").unwrap();
    let got: Vec<(char, String)> = changes
        .iter()
        .map(|c| (c.change.marker(), c.display_path()))
        .collect();
    let src = e.src.display().to_string();
    assert_eq!(
        got,
        [
            ('M', format!("{src}/d/edit")),
            ('-', format!("{src}/d/gone")),
            ('+', format!("{src}/d/new"))
        ]
    );
    assert_eq!(sum.bytes_delta, 7 + 1 - 1);

    // Live: nothing changed since b, then one new file.
    let live = || {
        scan_live(&e.id, &e.profile, &Progress::new(), &silent)
            .unwrap()
            .entries
            .into_iter()
            .map(Ok::<Entry, ManifestError>)
    };
    assert!(
        diff(m(&b), live(), b"").unwrap().0.is_empty(),
        "live == latest snapshot"
    );
    fs::write(e.src.join("d/later"), "l").unwrap();
    let (c, _) = diff(m(&b), live(), b"").unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].change, ChangeKind::Added);
}

#[test]
fn history_and_find() {
    let e = env();
    let f = e.src.join("notes.txt");
    fs::write(&f, "one").unwrap();
    let (s1, _) = e.snap(false);
    let (s2, _) = e.snap(false); // same version
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&f, "two!").unwrap();
    let (s3, _) = e.snap(false);
    fs::remove_file(&f).unwrap();
    e.snap(false); // absent
    fs::write(&f, "two!").unwrap(); // recreated: new mtime -> new version
    let (s5, _) = e.snap(false);

    let repo = open_repo(&e.id, &e.profile).unwrap();
    let snaps = repo.snapshots().unwrap();
    let rel = f
        .strip_prefix("/")
        .unwrap()
        .as_os_str()
        .as_encoded_bytes()
        .to_vec();
    let open = |s: &flux_fs::snapshot::SnapshotMeta| entries_of(&repo, s);
    let v = history::history(&open, &snaps, &rel).unwrap();
    assert_eq!(v.len(), 3, "{v:?}");
    assert_eq!(
        (v[0].first.as_str(), v[0].last.as_str(), v[0].snapshots),
        (s1.as_str(), s2.as_str(), 2)
    );
    assert_eq!((v[1].first.as_str(), v[1].size), (s3.as_str(), 4));
    assert_eq!(v[2].first, s5);

    let found = history::find(&open, &snaps, &|p: &[u8]| p.ends_with(b".txt")).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].versions, found[0].snapshots), (3, 4));
    assert_eq!(found[0].latest, s5);
}
