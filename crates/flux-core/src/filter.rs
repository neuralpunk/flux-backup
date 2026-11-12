//! Include/exclude decisions shared by all engines.
//!
//! Exclude patterns use gitignore syntax, evaluated against absolute paths:
//!
//! * `*.tmp`, `node_modules/`: no slash except a trailing one, so they match a name
//!   anywhere (a trailing `/` matches directories only)
//! * `/home/me/Downloads`: a leading `/` anchors the pattern to the filesystem root
//! * `~/Videos`: `~` expands to the user's home directory
//! * `**/build/out`: a slash in the middle anchors the pattern to the root, so use `**/` to
//!   match at any depth
//! * `!keep.tmp`: re-includes a path that an earlier pattern excluded
//!
//! With `exclude_gitignored`, a git working tree's own `.gitignore` files leave out what
//! they ignore too, as git reads them (relative to their folder, deeper files first, a
//! nested repository starting afresh). A profile's own `!pattern` still brings a path back.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::profile::Sources;
use crate::units::ByteSize;

const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

#[derive(Debug, thiserror::Error)]
#[error("invalid exclude pattern {pattern:?}: {message}")]
pub struct FilterError {
    pub pattern: String,
    pub message: String,
}

/// Compiled exclude rules for one profile.
#[derive(Debug, Clone)]
pub struct Filter {
    globs: Gitignore,
    /// Each pattern as written (`~/Videos`) by the line it became (`/home/me/Videos`).
    written: Vec<(String, String)>,
    /// Absolute paths that are always skipped (e.g. the backup repository itself).
    hard_excludes: Vec<PathBuf>,
    larger_than: Option<ByteSize>,
    exclude_caches: bool,
    if_present: Vec<String>,
    gitignore: bool,
}

/// The `.gitignore` rules that apply in one folder of a git working tree: its own file's,
/// then its parents', up to the repository's top folder.
#[derive(Debug)]
pub struct GitScope {
    parent: Option<Arc<GitScope>>,
    /// This folder's `.gitignore` (and its path), if it has one.
    rules: Option<(Gitignore, PathBuf)>,
}

impl GitScope {
    /// The `.gitignore` that leaves `abs` out, and the line that does, or `None` (not
    /// ignored, or brought back by a `!` line).
    pub fn ignored(&self, abs: &Path, is_dir: bool) -> Option<(&Path, String)> {
        let mut at = Some(self);
        while let Some(s) = at {
            if let Some((g, file)) = &s.rules {
                match g.matched(abs, is_dir) {
                    ignore::Match::Ignore(glob) => {
                        return Some((file.as_path(), glob.original().to_owned()));
                    }
                    ignore::Match::Whitelist(_) => return None,
                    ignore::Match::None => {}
                }
            }
            at = s.parent.as_deref();
        }
        None
    }
}

fn load_gitignore(dir: &Path) -> Option<(Gitignore, PathBuf)> {
    let file = dir.join(".gitignore");
    let mut b = GitignoreBuilder::new(dir);
    // A line git would also reject is skipped; the rest still count.
    let _ = b.add(&file);
    let g = b.build().ok()?;
    (!g.is_empty()).then_some((g, file))
}

impl Filter {
    /// Build a filter from a profile's sources section. `home` is used for `~` expansion.
    pub fn new(sources: &Sources, home: Option<&Path>) -> Result<Self, FilterError> {
        let mut b = GitignoreBuilder::new("/");
        let mut written = Vec::new();
        for pat in &sources.exclude {
            let expanded = expand_home(pat.trim(), home);
            if expanded.is_empty() {
                continue;
            }
            written.push((expanded.clone(), pat.clone()));
            b.add_line(None, &expanded).map_err(|e| FilterError {
                pattern: pat.clone(),
                message: e.to_string(),
            })?;
        }
        let globs = b.build().map_err(|e| FilterError {
            pattern: String::new(),
            message: e.to_string(),
        })?;
        Ok(Self {
            globs,
            written,
            hard_excludes: Vec::new(),
            larger_than: sources.exclude_larger_than,
            exclude_caches: sources.exclude_caches,
            if_present: sources.exclude_if_present.clone(),
            gitignore: sources.exclude_gitignored,
        })
    }

    /// Always skip `path` and everything below it.
    pub fn add_hard_exclude(&mut self, path: impl Into<PathBuf>) {
        self.hard_excludes.push(path.into());
    }

    /// Should this path be skipped, judged by its name and type alone?
    pub fn is_excluded(&self, abs: &Path, is_dir: bool) -> bool {
        self.excluded_in(None, abs, is_dir)
    }

    /// Should this path be skipped, with the `.gitignore` rules of the folder it's in
    /// (`git`, from [`Self::git_enter`] or [`Self::git_scope_of_dir`])? The profile's own
    /// patterns come first: a `!pattern` brings back what a `.gitignore` ignores.
    pub fn excluded_in(&self, git: Option<&GitScope>, abs: &Path, is_dir: bool) -> bool {
        if self.hard_excludes.iter().any(|h| abs == h) {
            return true;
        }
        match self.globs.matched(abs, is_dir) {
            ignore::Match::Ignore(_) => true,
            ignore::Match::Whitelist(_) => false,
            ignore::Match::None => git.is_some_and(|g| g.ignored(abs, is_dir).is_some()),
        }
    }

    /// Are `.gitignore` files read (`exclude_gitignored`)?
    pub fn reads_gitignore(&self) -> bool {
        self.gitignore
    }

    /// The `.gitignore` rules for the entries of `dir`, given those of its parent folder
    /// and whether `dir` holds `.git` (it's a repository's top folder: start afresh) and
    /// `.gitignore`. `None` outside a git working tree, or when `.gitignore` files aren't
    /// read.
    pub fn git_enter(
        &self,
        dir: &Path,
        has_git: bool,
        has_gitignore: bool,
        parent: Option<Arc<GitScope>>,
    ) -> Option<Arc<GitScope>> {
        if !self.gitignore {
            return None;
        }
        if has_git {
            return Some(Arc::new(GitScope {
                parent: None,
                rules: has_gitignore.then(|| load_gitignore(dir)).flatten(),
            }));
        }
        let parent = parent?;
        if !has_gitignore {
            return Some(parent);
        }
        match load_gitignore(dir) {
            Some(rules) => Some(Arc::new(GitScope {
                parent: Some(parent),
                rules: Some(rules),
            })),
            None => Some(parent),
        }
    }

    /// The `.gitignore` rules for the entries of `dir`, read from the disk: `dir` and the
    /// folders above it up to the nearest one with `.git`. For a walk's starting folders;
    /// the walk then uses [`Self::git_enter`].
    pub fn git_scope_of_dir(&self, dir: &Path) -> Option<Arc<GitScope>> {
        if !self.gitignore {
            return None;
        }
        let mut chain = Vec::new();
        let mut at = Some(dir);
        let mut top = None;
        while let Some(d) = at {
            chain.push(d);
            if d.join(".git").symlink_metadata().is_ok() {
                top = Some(d);
                break;
            }
            at = d.parent();
        }
        top?;
        let mut scope: Option<Arc<GitScope>> = None;
        for d in chain.into_iter().rev() {
            let has_gi = d.join(".gitignore").is_file();
            let has_git = scope.is_none();
            scope = self.git_enter(d, has_git, has_gi, scope);
        }
        scope
    }

    /// Should a regular file of `size` bytes be skipped?
    /// The pattern that decides `abs` (as written in the profile), and whether it leaves it
    /// out (`true`) or brings it back (`!pattern`, `false`). `None`: no pattern matches.
    pub fn rule_for(&self, abs: &Path, is_dir: bool) -> Option<(String, bool)> {
        let m = self.globs.matched(abs, is_dir);
        let glob = match &m {
            ignore::Match::Ignore(g) | ignore::Match::Whitelist(g) => *g,
            ignore::Match::None => return None,
        };
        let line = glob.original();
        let written = self
            .written
            .iter()
            .find(|(e, _)| e.trim() == line.trim())
            .map_or_else(|| line.to_owned(), |(_, w)| w.clone());
        Some((written, m.is_ignore()))
    }

    /// Paths always left out (the backup's own storage).
    pub fn is_hard_excluded(&self, abs: &Path) -> bool {
        self.hard_excludes.iter().any(|h| abs == h)
    }

    /// The size limit, if any.
    pub fn size_limit(&self) -> Option<u64> {
        self.larger_than.map(|b| b.bytes())
    }

    /// Why a folder's contents are left out by a marker inside it: `CACHEDIR.TAG`, or the
    /// marker file's name.
    pub fn marker_in<'a>(
        &self,
        dir: &Path,
        names: impl IntoIterator<Item = &'a [u8]>,
    ) -> Option<String> {
        for name in names {
            if self.exclude_caches
                && name == b"CACHEDIR.TAG"
                && is_cachedir_tag(&dir.join("CACHEDIR.TAG"))
            {
                return Some("CACHEDIR.TAG".into());
            }
            if self.if_present.iter().any(|m| m.as_bytes() == name) {
                return Some(String::from_utf8_lossy(name).into_owned());
            }
        }
        None
    }

    pub fn is_too_large(&self, size: u64) -> bool {
        self.larger_than.is_some_and(|max| size > max.bytes())
    }

    /// Should the contents of directory `dir` be skipped because of a marker file
    /// (`CACHEDIR.TAG` or one of `exclude_if_present`)?
    ///
    /// `names` are the directory's entry names, which the caller already has from
    /// reading the directory. This avoids extra `stat` calls.
    pub fn skip_dir_by_marker<'a>(
        &self,
        dir: &Path,
        names: impl IntoIterator<Item = &'a [u8]>,
    ) -> bool {
        self.marker_in(dir, names).is_some()
    }

    /// True if any marker rules are active (lets callers skip collecting names).
    pub fn has_markers(&self) -> bool {
        self.exclude_caches || !self.if_present.is_empty()
    }
}

fn expand_home(pat: &str, home: Option<&Path>) -> String {
    let (neg, body) = match pat.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", pat),
    };
    match (body.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{neg}{}{rest}", h.display())
        }
        _ => pat.to_owned(),
    }
}

/// A valid cache directory tag starts with the standard signature
/// (<https://bford.info/cachedir/>).
fn is_cachedir_tag(path: &Path) -> bool {
    let mut buf = [0u8; CACHEDIR_SIGNATURE.len()];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .map(|()| buf == CACHEDIR_SIGNATURE)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Filter {
        fn sources_for_tests(&self) -> Sources {
            Sources {
                paths: Vec::new(),
                exclude: Vec::new(),
                exclude_larger_than: None,
                exclude_caches: false,
                exclude_if_present: Vec::new(),
                one_file_system: false,
                exclude_gitignored: false,
                volumes: Vec::new(),
            }
        }
    }

    fn filter(patterns: &[&str]) -> Filter {
        let sources = Sources {
            paths: vec!["/home/me".into()],
            exclude: patterns.iter().map(|s| s.to_string()).collect(),
            exclude_larger_than: Some(ByteSize(100)),
            exclude_caches: true,
            exclude_if_present: vec![".nobackup".into()],
            one_file_system: false,
            exclude_gitignored: false,
            volumes: Vec::new(),
        };
        Filter::new(&sources, Some(Path::new("/home/me"))).unwrap()
    }

    #[test]
    fn glob_semantics() {
        let f = filter(&[
            "*.tmp",
            ".cache/",
            "/home/me/Downloads",
            "~/Videos",
            "**/target/debug",
            "!keep.tmp",
        ]);
        assert!(f.is_excluded(Path::new("/home/me/a.tmp"), false));
        assert!(f.is_excluded(Path::new("/home/me/deep/b.tmp"), false));
        assert!(!f.is_excluded(Path::new("/home/me/keep.tmp"), false));
        assert!(f.is_excluded(Path::new("/home/me/.cache"), true));
        assert!(
            !f.is_excluded(Path::new("/home/me/.cache"), false),
            "dir-only pattern"
        );
        assert!(f.is_excluded(Path::new("/home/me/Downloads"), true));
        assert!(!f.is_excluded(Path::new("/home/other/Downloads"), true));
        assert!(f.is_excluded(Path::new("/home/me/Videos"), true));
        assert!(f.is_excluded(Path::new("/home/me/src/x/target/debug"), true));
        assert!(!f.is_excluded(Path::new("/home/me/src/x/target/release"), true));
        assert!(f.is_too_large(101));
        assert!(!f.is_too_large(100));
    }

    #[test]
    fn which_rule_decides() {
        let f = filter(&["*.tmp", "~/Videos", "!keep.tmp"]);
        assert_eq!(
            f.rule_for(Path::new("/home/me/x/a.tmp"), false),
            Some(("*.tmp".into(), true))
        );
        assert_eq!(
            f.rule_for(Path::new("/home/me/Videos"), true),
            Some(("~/Videos".into(), true)),
            "as written, not expanded"
        );
        assert_eq!(
            f.rule_for(Path::new("/home/me/keep.tmp"), false),
            Some(("!keep.tmp".into(), false))
        );
        assert_eq!(f.rule_for(Path::new("/home/me/a.txt"), false), None);
        assert_eq!(f.size_limit(), Some(100));
    }

    #[test]
    fn hard_excludes() {
        let mut f = filter(&[]);
        f.add_hard_exclude("/home/me/backup");
        assert!(f.is_excluded(Path::new("/home/me/backup"), true));
        assert!(!f.is_excluded(Path::new("/home/me/backup2"), true));
    }

    #[test]
    fn markers() {
        let tmp = tempfile::tempdir().unwrap();
        let f = filter(&[]);
        let d = tmp.path();
        assert!(!f.skip_dir_by_marker(d, [b"a".as_slice()]));
        assert!(f.skip_dir_by_marker(d, [b".nobackup".as_slice()]));
        fs::write(d.join("CACHEDIR.TAG"), b"not a tag").unwrap();
        assert!(!f.skip_dir_by_marker(d, [b"CACHEDIR.TAG".as_slice()]));
        fs::write(
            d.join("CACHEDIR.TAG"),
            [CACHEDIR_SIGNATURE, b"\n# cache"].concat(),
        )
        .unwrap();
        assert!(f.skip_dir_by_marker(d, [b"CACHEDIR.TAG".as_slice()]));
    }

    /// A git working tree: `.gitignore` at the top and in a subfolder, a nested
    /// repository with its own, and a `.gitignore` outside any repository.
    fn repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join("proj");
        for d in [".git", "sub", "inner/.git", "target"] {
            fs::create_dir_all(r.join(d)).unwrap();
        }
        fs::write(r.join(".gitignore"), "target/\n*.log\n!keep.log\n.env\n").unwrap();
        fs::write(r.join("sub/.gitignore"), "secret.txt\n").unwrap();
        fs::write(r.join("inner/.gitignore"), "only-inner.txt\n").unwrap();
        fs::write(t.path().join(".gitignore"), "*\n").unwrap();
        t
    }

    fn git_filter(on: bool, patterns: &[&str]) -> Filter {
        let mut s = Sources {
            paths: vec!["/".into()],
            exclude: patterns.iter().map(|s| s.to_string()).collect(),
            ..filter(&[]).sources_for_tests()
        };
        s.exclude_gitignored = on;
        Filter::new(&s, None).unwrap()
    }

    #[test]
    fn gitignore_files_leave_out_what_git_ignores() {
        let t = repo();
        let r = t.path().join("proj");
        let f = git_filter(true, &[]);
        let top = f.git_scope_of_dir(&r).expect("a working tree");
        let out = |dir: &Path, name: &str, is_dir: bool| {
            let scope = f.git_scope_of_dir(dir);
            f.excluded_in(scope.as_deref(), &dir.join(name), is_dir)
        };
        assert!(out(&r, "target", true));
        assert!(out(&r, "build.log", false));
        assert!(!out(&r, "keep.log", false), "a ! line brings it back");
        assert!(out(&r, ".env", false));
        assert!(!out(&r, "main.rs", false));
        assert!(
            out(&r.join("sub"), "secret.txt", false),
            "a subfolder's own file"
        );
        assert!(out(&r.join("sub"), "x.log", false), "and its parents'");
        assert!(
            !out(&r, "secret.txt", false),
            "a subfolder's rules stay there"
        );
        // A nested repository starts afresh: the outer *.log doesn't reach it.
        assert!(!out(&r.join("inner"), "x.log", false));
        assert!(out(&r.join("inner"), "only-inner.txt", false));
        // Outside a working tree, a .gitignore means nothing.
        assert!(f.git_scope_of_dir(t.path()).is_none());
        assert!(!out(t.path(), "anything", false));
        // Which file and line, for the rule tester.
        let (file, line) = top.ignored(&r.join("a.log"), false).unwrap();
        assert_eq!(
            (file, line.as_str()),
            (r.join(".gitignore").as_path(), "*.log")
        );
    }

    #[test]
    fn the_backups_own_rules_come_first_and_the_switch_is_off_by_default() {
        let t = repo();
        let r = t.path().join("proj");
        let f = git_filter(true, &["!.env"]);
        let scope = f.git_scope_of_dir(&r);
        assert!(!f.excluded_in(scope.as_deref(), &r.join(".env"), false));
        let off = git_filter(false, &[]);
        assert!(off.git_scope_of_dir(&r).is_none());
        assert!(!off.excluded_in(None, &r.join("target"), true));
    }
}
