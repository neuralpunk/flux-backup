//! What a backup takes, and why: the rule tester ([`explain`]: one path, and the rule,
//! limit or marker that decides it) and coverage ([`list`]: a folder's items each marked
//! backed up or left out, with totals of what's in and out below each one).
//!
//! The decisions are the scanner's ([`crate::scan`]), in its order: the source folders,
//! the backup's own storage, exclusion patterns (top-down: a folder left out takes
//! everything in it), the size limit, cache and marker files, other disks.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

use flux_core::filter::{Filter, FilterError};
use flux_core::profile::{Engine, Profile, Target};

/// Why a path is in or out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Why {
    /// Backed up: inside `source`, nothing leaves it out. `rule`: a `!pattern` that brought
    /// it back.
    Included {
        source: PathBuf,
        rule: Option<String>,
    },
    /// Not inside any folder the backup takes.
    NotInSources,
    /// An exclusion pattern, matching `at` (the path or a folder it's in).
    Excluded { rule: String, at: PathBuf },
    /// The backup's own storage.
    Storage { at: PathBuf },
    /// Larger than the size limit.
    TooLarge { size: u64, limit: u64 },
    /// In a folder marked as a cache (`CACHEDIR.TAG`) or with a marker file.
    Marker { name: String, at: PathBuf },
    /// On another disk mounted at `at` ("stay on one disk").
    OtherDisk { at: PathBuf },
    /// A git working tree's `.gitignore` (`file`, its `line`) leaves out `at` (the path or a
    /// folder it's in).
    GitIgnored {
        file: PathBuf,
        line: String,
        at: PathBuf,
    },
}

impl Why {
    pub fn included(&self) -> bool {
        matches!(self, Why::Included { .. })
    }

    /// In words, for people.
    pub fn text(&self, path: &Path) -> String {
        let here = |at: &Path| at == path;
        match self {
            Why::Included { source, rule: None } => {
                format!("Backed up: it's in {}.", source.display())
            }
            Why::Included {
                source,
                rule: Some(r),
            } => format!(
                "Backed up: it's in {}, and the rule {r} brings it back.",
                source.display()
            ),
            Why::NotInSources => {
                "Not backed up: it isn't in any of the folders this backup takes.".into()
            }
            Why::Excluded { rule, at } if here(at) => format!("Left out by the rule {rule}."),
            Why::Excluded { rule, at } => format!(
                "Left out: it's in {}, which the rule {rule} leaves out.",
                at.display()
            ),
            Why::Storage { at } => format!(
                "Left out: {} is where this backup keeps its own data.",
                at.display()
            ),
            Why::TooLarge { size, limit } => format!(
                "Left out: {} is over the size limit of {}.",
                flux_core::ByteSize(*size).display_approx(),
                flux_core::ByteSize(*limit).display_approx()
            ),
            Why::Marker { name, at } if name == "CACHEDIR.TAG" => format!(
                "Left out: {} is marked as a cache folder (it holds a CACHEDIR.TAG).",
                at.display()
            ),
            Why::Marker { name, at } => format!(
                "Left out: {} holds {name}, which marks folders to leave out.",
                at.display()
            ),
            Why::OtherDisk { at } => format!(
                "Left out: {} is another disk, and this backup stays on one disk.",
                at.display()
            ),
            Why::GitIgnored { file, line, at } if here(at) => format!(
                "Left out: {} ignores it ({line}), and this backup skips what .gitignore files ignore.",
                file.display()
            ),
            Why::GitIgnored { file, line, at } => format!(
                "Left out: it's in {}, which {} ignores ({line}), and this backup skips what .gitignore files ignore.",
                at.display(),
                file.display()
            ),
        }
    }
}

/// The rules of one backup.
pub struct Rules {
    filter: Filter,
    sources: Vec<PathBuf>,
    one_disk: bool,
    /// The `.gitignore` rules of each folder looked at (when they're read).
    git: std::sync::Mutex<
        std::collections::HashMap<PathBuf, Option<std::sync::Arc<flux_core::filter::GitScope>>>,
    >,
}

/// The folders a backup never takes because it keeps its own data there.
pub fn storage_of(p: &Profile) -> Vec<PathBuf> {
    let Target::Local { path, .. } = &p.target else {
        return Vec::new();
    };
    match p.engine {
        Engine::Mirror | Engine::Vault => vec![path.join("flux-backup"), path.join("flux-vault")],
        Engine::Rsync => vec![path.join("flux-rsync")],
        Engine::Borg => vec![path.join("flux-borg")],
    }
}

impl Rules {
    pub fn of(p: &Profile) -> Result<Self, FilterError> {
        let mut filter = Filter::new(&p.sources, flux_core::sys::home_dir().as_deref())?;
        for n in storage_of(p) {
            filter.add_hard_exclude(n);
        }
        Ok(Rules {
            filter,
            sources: p.sources.paths.clone(),
            one_disk: p.sources.one_file_system,
            git: Default::default(),
        })
    }

    /// The `.gitignore` that leaves out `path` (in folder `path`'s parent), if any.
    fn git_ignored(&self, path: &Path, is_dir: bool) -> Option<(PathBuf, String)> {
        if !self.filter.reads_gitignore() {
            return None;
        }
        let dir = path.parent()?;
        let scope = {
            let mut cache = self.git.lock().unwrap_or_else(|e| e.into_inner());
            cache
                .entry(dir.to_path_buf())
                .or_insert_with(|| self.filter.git_scope_of_dir(dir))
                .clone()
        }?;
        scope
            .ignored(path, is_dir)
            .map(|(f, line)| (f.to_path_buf(), line))
    }

    /// The source folder `path` is in (the innermost), if any.
    fn source_of(&self, path: &Path) -> Option<&PathBuf> {
        self.sources
            .iter()
            .filter(|s| path.starts_with(s))
            .max_by_key(|s| s.as_os_str().len())
    }

    /// The verdict on `path` itself, its parent being in (`parent_dev`: its disk). `meta`
    /// `None`: it doesn't exist (judged as a file, or a folder when `is_dir`).
    fn own(
        &self,
        path: &Path,
        meta: Option<&fs::Metadata>,
        is_dir: bool,
        root_dev: Option<u64>,
    ) -> Option<Why> {
        if self.filter.is_hard_excluded(path) {
            return Some(Why::Storage { at: path.into() });
        }
        match self.filter.rule_for(path, is_dir) {
            Some((rule, true)) => {
                return Some(Why::Excluded {
                    rule,
                    at: path.into(),
                });
            }
            // A `!pattern` of the backup's own brings it back, whatever .gitignore says.
            Some((_, false)) => {}
            None => {
                if let Some((file, line)) = self.git_ignored(path, is_dir) {
                    return Some(Why::GitIgnored {
                        file,
                        line,
                        at: path.into(),
                    });
                }
            }
        }
        if let Some(m) = meta {
            if m.is_file()
                && let Some(limit) = self.filter.size_limit()
                && self.filter.is_too_large(m.len())
            {
                return Some(Why::TooLarge {
                    size: m.len(),
                    limit,
                });
            }
            if m.is_dir() {
                if self.one_disk && root_dev.is_some_and(|d| d != m.dev()) {
                    return Some(Why::OtherDisk { at: path.into() });
                }
                if self.filter.has_markers()
                    && let Ok(rd) = fs::read_dir(path)
                {
                    let names: Vec<_> = rd.flatten().map(|e| e.file_name()).collect();
                    if let Some(name) = self.filter.marker_in(
                        path,
                        names
                            .iter()
                            .map(|n| std::os::unix::ffi::OsStrExt::as_bytes(n.as_os_str())),
                    ) {
                        return Some(Why::Marker {
                            name,
                            at: path.into(),
                        });
                    }
                }
            }
        }
        None
    }
}

/// Is `path` (absolute) backed up, and why? A path that doesn't exist is judged as it
/// would be (a folder when written with a trailing `/`).
pub fn explain(rules: &Rules, path: &Path) -> Why {
    let as_dir = path.as_os_str().as_encoded_bytes().ends_with(b"/");
    let path: PathBuf = path.components().collect();
    let Some(source) = rules.source_of(&path).cloned() else {
        return Why::NotInSources;
    };
    let root_dev = fs::metadata(&source).ok().map(|m| m.dev());
    // From the source down to the path: the first folder (or the path) left out decides.
    let mut at = source.clone();
    let rest: Vec<_> = path
        .strip_prefix(&source)
        .unwrap_or(Path::new(""))
        .components()
        .collect();
    let mut steps = vec![at.clone()];
    for c in &rest {
        at.push(c);
        steps.push(at.clone());
    }
    for (i, step) in steps.iter().enumerate() {
        let last = i + 1 == steps.len();
        let meta = fs::symlink_metadata(step).ok();
        let is_dir = meta.as_ref().map_or(!last || as_dir, |m| m.is_dir());
        // (A marked folder or a mount point is kept as an empty folder; what matters, its
        // contents, is left out, so it counts as left out.)
        if let Some(why) = rules.own(step, meta.as_ref(), is_dir, root_dev) {
            return why;
        }
    }
    let is_dir = fs::symlink_metadata(&path).map_or(as_dir, |m| m.is_dir());
    let rule = match rules.filter.rule_for(&path, is_dir) {
        Some((r, false)) => Some(r),
        _ => None,
    };
    Why::Included { source, rule }
}

/// One item of a folder, for coverage.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub dir: bool,
    pub size: u64,
    pub backed_up: bool,
    pub why: String,
    /// What's in and out below a folder (the folder itself for a file).
    pub files_in: u64,
    pub bytes_in: u64,
    pub files_out: u64,
    pub bytes_out: u64,
}

/// A folder's items, each marked.
#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub path: PathBuf,
    pub backed_up: bool,
    pub why: String,
    pub items: Vec<Item>,
    /// The totals stopped at the time limit (they show what was counted).
    pub partial: bool,
}

/// The items of folder `dir`, marked in or out with totals below each folder, counting
/// until `deadline`.
pub fn list(rules: &Rules, dir: &Path, deadline: Option<Instant>) -> std::io::Result<Coverage> {
    let dir: PathBuf = dir.components().collect();
    let here = explain(rules, &dir);
    let root_dev = rules
        .source_of(&dir)
        .and_then(|s| fs::metadata(s).ok())
        .map(|m| m.dev());
    let mut partial = false;
    let mut items = Vec::new();
    // Inside a folder left out, or a cache folder / mount point kept empty: all out.
    let contents_out = match &here {
        Why::Included { .. } => {
            let meta = fs::symlink_metadata(&dir).ok();
            rules.own(&dir, meta.as_ref(), true, root_dev).is_some()
        }
        _ => true,
    };
    let mut rd: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
    rd.sort_by_key(|e| e.file_name());
    for e in rd {
        let path = e.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let why = if contents_out {
            explain(rules, &path)
        } else if rules.source_of(&path).is_none() {
            Why::NotInSources
        } else {
            match rules.own(&path, Some(&meta), is_dir, root_dev) {
                Some(w) => w,
                None => explain(rules, &path),
            }
        };
        let backed_up = why.included();
        let mut t = Totals::default();
        if is_dir {
            // A marked folder or mount point is kept itself; its contents are out.
            let descend_in = backed_up && rules.own(&path, Some(&meta), true, root_dev).is_none();
            partial |= !count(rules, &path, descend_in, root_dev, deadline, &mut t);
        } else if backed_up {
            (t.files_in, t.bytes_in) = (1, meta.len());
        } else {
            (t.files_out, t.bytes_out) = (1, meta.len());
        }
        items.push(Item {
            name: e.file_name().to_string_lossy().into_owned(),
            why: why.text(&path),
            path,
            dir: is_dir,
            size: if is_dir { 0 } else { meta.len() },
            backed_up,
            files_in: t.files_in,
            bytes_in: t.bytes_in,
            files_out: t.files_out,
            bytes_out: t.bytes_out,
        });
    }
    Ok(Coverage {
        backed_up: here.included(),
        why: here.text(&dir),
        path: dir,
        items,
        partial,
    })
}

#[derive(Default)]
struct Totals {
    files_in: u64,
    bytes_in: u64,
    files_out: u64,
    bytes_out: u64,
}

/// Count the files below `dir` (taken: `inside`). `false`: stopped at the deadline.
fn count(
    rules: &Rules,
    dir: &Path,
    inside: bool,
    root_dev: Option<u64>,
    deadline: Option<Instant>,
    t: &mut Totals,
) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return true;
    };
    for e in rd.flatten() {
        if deadline.is_some_and(|d| Instant::now() > d) {
            return false;
        }
        let path = e.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let in_ = inside
            && rules
                .own(&path, Some(&meta), meta.is_dir(), root_dev)
                .is_none();
        if meta.is_dir() {
            // Out because of a marker or another disk: the folder stays, its contents go.
            if !count(rules, &path, in_, root_dev, deadline, t) {
                return false;
            }
        } else if in_ {
            t.files_in += 1;
            t.bytes_in += meta.len();
        } else {
            t.files_out += 1;
            t.bytes_out += meta.len();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Profile) {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let home = r.join("home");
        for d in [
            "docs/deep",
            "proj/node_modules/x",
            "cache",
            "tgt",
            "big",
            "skip",
        ] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        fs::write(home.join("docs/a.txt"), "a").unwrap();
        fs::write(home.join("docs/deep/b.tmp"), "bb").unwrap();
        fs::write(home.join("docs/deep/keep.tmp"), "kk").unwrap();
        fs::write(home.join("proj/main.rs"), "fn main() {}").unwrap();
        fs::write(home.join("proj/node_modules/x/lib.js"), "x".repeat(50)).unwrap();
        fs::write(
            home.join("cache/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        fs::write(home.join("cache/blob"), "c").unwrap();
        fs::write(home.join("big/huge.iso"), vec![0u8; 2000]).unwrap();
        fs::write(home.join("skip/.nobackup"), "").unwrap();
        fs::write(home.join("skip/x"), "x").unwrap();
        let mut p = Profile::new(
            "T",
            Target::Local {
                path: home.join("tgt"),
                fs_uuid: None,
            },
            vec![home.clone()],
        );
        p.sources.exclude = vec!["*.tmp".into(), "!keep.tmp".into(), "node_modules/".into()];
        p.sources.exclude_larger_than = Some(flux_core::ByteSize(1000));
        p.sources.exclude_if_present = vec![".nobackup".into()];
        (t, p)
    }

    #[test]
    fn the_rule_tester_names_the_gitignore() {
        let (t, mut p) = setup();
        let proj = t.path().join("home/proj");
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::create_dir_all(proj.join("dist/js")).unwrap();
        fs::write(proj.join(".gitignore"), "dist/\n.env\n").unwrap();
        fs::write(proj.join(".env"), "KEY=1").unwrap();
        fs::write(proj.join("dist/js/app.js"), "x").unwrap();
        p.sources.exclude_gitignored = true;
        let r = Rules::of(&p).unwrap();
        let w = explain(&r, &proj.join(".env"));
        assert_eq!(
            w,
            Why::GitIgnored {
                file: proj.join(".gitignore"),
                line: ".env".into(),
                at: proj.join(".env")
            }
        );
        assert!(
            w.text(&proj.join(".env"))
                .contains(".gitignore ignores it (.env)")
        );
        // In a folder it ignores: that folder is named.
        let w = explain(&r, &proj.join("dist/js/app.js"));
        assert!(
            matches!(&w, Why::GitIgnored { at, .. } if *at == proj.join("dist")),
            "{w:?}"
        );
        // The listing marks it out.
        let c = list(&r, &proj, None).unwrap();
        let env = c.items.iter().find(|i| i.name == ".env").unwrap();
        assert!(!env.backed_up);
        assert!(
            c.items
                .iter()
                .find(|i| i.name == "main.rs")
                .unwrap()
                .backed_up
        );
        // Off: backed up.
        p.sources.exclude_gitignored = false;
        assert!(explain(&Rules::of(&p).unwrap(), &proj.join(".env")).included());
    }

    #[test]
    fn explain_names_the_deciding_rule() {
        let (t, p) = setup();
        let home = t.path().join("home");
        let r = Rules::of(&p).unwrap();
        assert_eq!(
            explain(&r, &home.join("docs/a.txt")),
            Why::Included {
                source: home.clone(),
                rule: None
            }
        );
        assert_eq!(
            explain(&r, &home.join("docs/deep/b.tmp")),
            Why::Excluded {
                rule: "*.tmp".into(),
                at: home.join("docs/deep/b.tmp")
            }
        );
        assert_eq!(
            explain(&r, &home.join("docs/deep/keep.tmp")),
            Why::Included {
                source: home.clone(),
                rule: Some("!keep.tmp".into())
            }
        );
        // A folder left out takes what's in it.
        let w = explain(&r, &home.join("proj/node_modules/x/lib.js"));
        assert_eq!(
            w,
            Why::Excluded {
                rule: "node_modules/".into(),
                at: home.join("proj/node_modules")
            }
        );
        assert!(
            w.text(&home.join("proj/node_modules/x/lib.js"))
                .contains("which the rule node_modules/ leaves out")
        );
        assert!(matches!(
            explain(&r, &home.join("big/huge.iso")),
            Why::TooLarge {
                size: 2000,
                limit: 1000
            }
        ));
        assert!(
            matches!(explain(&r, &home.join("cache/blob")), Why::Marker { ref name, .. } if name == "CACHEDIR.TAG")
        );
        assert!(
            matches!(explain(&r, &home.join("cache")), Why::Marker { .. }),
            "the same as the list says"
        );
        assert!(
            matches!(explain(&r, &home.join("skip/x")), Why::Marker { ref name, .. } if name == ".nobackup")
        );
        assert!(matches!(
            explain(&r, &home.join("tgt/flux-backup/x")),
            Why::Storage { .. }
        ));
        assert_eq!(explain(&r, Path::new("/etc/passwd")), Why::NotInSources);
        // Not there (yet): judged by the rules.
        assert!(matches!(
            explain(&r, &home.join("new/x.tmp")),
            Why::Excluded { .. }
        ));
        assert!(explain(&r, &home.join("new/x.txt")).included());
    }

    /// Every file the scanner takes is "backed up" here, and every other file isn't.
    #[test]
    fn agrees_with_the_scanner() {
        let (t, p) = setup();
        let home = t.path().join("home");
        fs::create_dir_all(home.join("tgt/flux-backup/old")).unwrap();
        fs::write(home.join("tgt/flux-backup/old/f"), "stored").unwrap();
        let r = Rules::of(&p).unwrap();
        let scanned = crate::scan::scan_profile(
            &p,
            &storage_of(&p),
            &flux_core::progress::Progress::new(),
            &flux_core::progress::silent,
        )
        .unwrap();
        let taken: std::collections::HashSet<PathBuf> =
            scanned.entries.iter().map(|e| e.abs_path()).collect();
        let mut stack = vec![home.clone()];
        let mut files = 0;
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let path = e.path();
                if e.file_type().unwrap().is_dir() {
                    stack.push(path);
                } else {
                    files += 1;
                    assert_eq!(
                        explain(&r, &path).included(),
                        taken.contains(&path),
                        "{}: {:?}",
                        path.display(),
                        explain(&r, &path)
                    );
                }
            }
        }
        assert!(files >= 10);
    }

    #[test]
    fn coverage_marks_items_and_totals() {
        let (t, p) = setup();
        let home = t.path().join("home");
        let r = Rules::of(&p).unwrap();
        let c = list(&r, &home, None).unwrap();
        assert!(c.backed_up && !c.partial);
        let item = |n: &str| c.items.iter().find(|i| i.name == n).unwrap();
        let proj = item("proj");
        assert!(proj.backed_up && proj.dir);
        assert_eq!((proj.files_in, proj.files_out, proj.bytes_out), (1, 1, 50));
        let docs = item("docs");
        assert_eq!(
            (docs.files_in, docs.files_out),
            (2, 1),
            "a.txt and keep.tmp in, b.tmp out"
        );
        let cache = item("cache");
        assert!(
            !cache.backed_up && cache.why.contains("cache folder"),
            "{}",
            cache.why
        );
        assert_eq!((cache.files_in, cache.files_out), (0, 2));
        let big = item("big");
        assert_eq!((big.files_in, big.files_out, big.bytes_out), (0, 1, 2000));
        // Inside a folder left out: everything is out, and says why.
        let nm = list(&r, &home.join("proj/node_modules"), None).unwrap();
        assert!(!nm.backed_up);
        assert!(
            nm.items
                .iter()
                .all(|i| !i.backed_up && i.why.contains("node_modules/"))
        );
        // A deadline already passed: partial totals.
        let c = list(&r, &home, Some(Instant::now())).unwrap();
        assert!(c.partial);
    }
}
