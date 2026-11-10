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

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

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
        })
    }

    /// Always skip `path` and everything below it.
    pub fn add_hard_exclude(&mut self, path: impl Into<PathBuf>) {
        self.hard_excludes.push(path.into());
    }

    /// Should this path be skipped, judged by its name and type alone?
    pub fn is_excluded(&self, abs: &Path, is_dir: bool) -> bool {
        if self.hard_excludes.iter().any(|h| abs == h) {
            return true;
        }
        self.globs.matched(abs, is_dir).is_ignore()
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

    fn filter(patterns: &[&str]) -> Filter {
        let sources = Sources {
            paths: vec!["/home/me".into()],
            exclude: patterns.iter().map(|s| s.to_string()).collect(),
            exclude_larger_than: Some(ByteSize(100)),
            exclude_caches: true,
            exclude_if_present: vec![".nobackup".into()],
            one_file_system: false,
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
}
