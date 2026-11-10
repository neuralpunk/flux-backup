//! Property tests: the exclusion rules mean the same thing however they're carried out.
//!
//! - Flux's gitignore-style patterns, translated to rsync filter rules (compatibility
//!   mode), leave out exactly the files Flux's own scan leaves out.
//! - Back In Time's rsync patterns, converted to Flux patterns (importing a Back In Time
//!   profile), leave out exactly what rsync leaves out with them.
//!
//! Random trees and patterns; each case is checked against the real `rsync --dry-run`
//! and Flux's scanner. Skipped when rsync isn't installed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use proptest::prelude::*;

use flux_core::profile::{Profile, Target};

const NAMES: &[&str] = &[
    "a",
    "b",
    "build",
    "node_modules",
    "x.tmp",
    "y.log",
    "keep.tmp",
    "doc.txt",
    ".cache",
    "c d",
];

fn rsync_available() -> bool {
    Command::new("rsync")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Files of a tree: paths of 1 to 3 names.
fn tree() -> impl Strategy<Value = BTreeSet<Vec<usize>>> {
    proptest::collection::btree_set(proptest::collection::vec(0..NAMES.len(), 1..=3), 1..14)
}

/// Make the tree under `src`. A path that is a prefix of another becomes a folder.
fn make(src: &Path, files: &BTreeSet<Vec<usize>>) -> Vec<PathBuf> {
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|p| p.iter().map(|&i| NAMES[i]).collect())
        .collect();
    let dirs: BTreeSet<PathBuf> = paths
        .iter()
        .flat_map(|p| {
            p.ancestors()
                .skip(1)
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        })
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    let mut out = Vec::new();
    for d in &dirs {
        fs::create_dir_all(src.join(d)).unwrap();
    }
    for p in &paths {
        if dirs.contains(p) {
            continue;
        }
        fs::write(src.join(p), p.to_string_lossy().as_bytes()).unwrap();
        out.push(p.clone());
    }
    out
}

/// The files (relative to `src`) rsync would copy with these arguments, run as the
/// compatibility mode does: `--relative`, an absolute source.
fn rsync_files(src: &Path, dest: &Path, extra: &[String]) -> BTreeSet<PathBuf> {
    let o = Command::new("rsync")
        .args(["--archive", "--relative", "--dry-run", "--out-format=%n"])
        .args(extra)
        .arg(src)
        .arg(dest)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // With --relative, rsync names every path from the root.
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| !l.ends_with('/'))
        .filter_map(|l| Path::new(l).strip_prefix(src).ok().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

/// The files Flux's scanner takes with these exclusions.
fn flux_files(src: &Path, exclude: &[String]) -> BTreeSet<PathBuf> {
    let mut p = Profile::new(
        "t",
        Target::Local {
            path: "/nonexistent".into(),
            fs_uuid: None,
        },
        vec![src.to_path_buf()],
    );
    p.sources.exclude = exclude.to_vec();
    p.sources.exclude_caches = false;
    let r = flux_fs::scan::scan_profile(
        &p,
        &[],
        &flux_core::progress::Progress::new(),
        &flux_core::progress::silent,
    )
    .unwrap();
    r.entries
        .iter()
        .filter(|e| e.kind == flux_fs::entry::Kind::File)
        .filter_map(|e| e.abs_path().strip_prefix(src).ok().map(Path::to_path_buf))
        .collect()
}

/// A Flux (gitignore-style) pattern; `{src}` stands for the source folder's path.
fn flux_pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        ext().prop_map(|e| format!("*.{e}")),
        name().prop_map(|n| n.to_owned()),
        name().prop_map(|n| format!("{n}/")),
        name().prop_map(|n| format!("!{n}")),
        (name(), name()).prop_map(|(a, b)| format!("{{src}}/{a}/{b}")),
        name().prop_map(|n| format!("{{src}}/{n}")),
        (name(), name()).prop_map(|(a, b)| format!("**/{a}/{b}")),
        Just("!keep.tmp".to_owned()),
        Just("?.tmp".to_owned()),
        Just("[xy].*".to_owned()),
        Just("*".to_owned()),
        name().prop_map(|n| format!("**/{n}/**")),
        name().prop_map(|n| format!("**/{n}")),
        name().prop_map(|n| format!("{{src}}/**/{n}")),
        name().prop_map(|n| format!("!{n}/")),
    ]
}

/// A Back In Time (rsync) exclude pattern; `{src}` stands for the source folder's path.
fn bit_pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        ext().prop_map(|e| format!("*.{e}")),
        name().prop_map(|n| n.to_owned()),
        name().prop_map(|n| format!("{n}/")),
        (name(), name()).prop_map(|(a, b)| format!("{a}/{b}")),
        name().prop_map(|n| format!("{{src}}/{n}")),
        name().prop_map(|n| format!("**/{n}")),
        name().prop_map(|n| format!("{n}/***")),
        Just("?.tmp".to_owned()),
        Just("[xy].*".to_owned()),
        name().prop_map(|n| format!("{{src}}/**/{n}")),
        (name(), name()).prop_map(|(a, b)| format!("{a}/*/{b}")),
    ]
}

fn name() -> impl Strategy<Value = &'static str> {
    proptest::sample::select(NAMES)
}

fn ext() -> impl Strategy<Value = &'static str> {
    proptest::sample::select(&["tmp", "log", "txt"][..])
}

fn workdir() -> tempfile::TempDir {
    // A name no pattern can match, so only the tree itself is judged.
    tempfile::Builder::new()
        .prefix("fluxprop")
        .tempdir()
        .unwrap()
}

proptest! {
    // FLUX_PROP_CASES raises the number of cases for a longer search.
    #![proptest_config(ProptestConfig {
        cases: std::env::var("FLUX_PROP_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(96),
        ..ProptestConfig::default()
    })]

    /// Flux's exclusions, as the rsync rules file the compatibility mode writes.
    #[test]
    fn rsync_rules_leave_out_what_flux_leaves_out(
        files in tree(),
        patterns in proptest::collection::vec(flux_pattern(), 0..5),
    ) {
        if !rsync_available() {
            return Ok(());
        }
        let t = workdir();
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        make(&src, &files);
        let exclude: Vec<String> = patterns
            .iter()
            .map(|p| p.replace("{src}", src.to_str().unwrap()))
            .collect();
        let mut sources = flux_core::profile::Sources {
            paths: vec![src.clone()],
            exclude: exclude.clone(),
            exclude_larger_than: None,
            exclude_caches: false,
            exclude_if_present: Vec::new(),
            one_file_system: false,
            volumes: Vec::new(),
        };
        sources.exclude = exclude.clone();
        let rules = flux_compat::filter::RuleSet::new(&sources, None, Vec::new()).render("test");
        let rules_file = t.path().join("rules");
        fs::write(&rules_file, &rules).unwrap();
        let via_rsync = rsync_files(
            &src,
            &t.path().join("dest"),
            &[format!("--exclude-from={}", rules_file.display())],
        );
        let via_flux = flux_files(&src, &exclude);
        prop_assert_eq!(via_rsync, via_flux, "patterns {:?}\nrules:\n{}", exclude, rules);
    }

    /// Back In Time's exclusions, converted when its profile is imported.
    #[test]
    fn back_in_time_patterns_mean_the_same_in_flux(
        files in tree(),
        patterns in proptest::collection::vec(bit_pattern(), 0..5),
    ) {
        if !rsync_available() {
            return Ok(());
        }
        let t = workdir();
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        make(&src, &files);
        let bit: Vec<String> = patterns
            .iter()
            .map(|p| p.replace("{src}", src.to_str().unwrap()))
            .collect();
        let args: Vec<String> = bit.iter().map(|p| format!("--exclude={p}")).collect();
        let via_rsync = rsync_files(&src, &t.path().join("dest"), &args);
        let converted: Vec<String> = bit
            .iter()
            .filter_map(|p| flux_compat::backintime::pattern(p))
            .collect();
        let via_flux = flux_files(&src, &converted);
        prop_assert_eq!(via_rsync, via_flux, "Back In Time {:?} became {:?}", bit, converted);
    }
}
