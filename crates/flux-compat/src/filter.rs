//! Profile exclusions in rsync's own filter syntax.
//!
//! Flux's patterns are gitignore-style and evaluated against absolute paths, where the
//! *last* matching pattern wins. rsync's filter rules are evaluated first-match-wins, so
//! the translated rules come in reverse order. Backups run with `--relative` and absolute
//! sources, so rsync's transfer root is `/` and anchored patterns mean the same thing in
//! both. Paths are matched exactly the way rsync matches them; the rules file is written
//! next to every snapshot.
//!
//! rsync has no "skip folders that contain a marker file" option. When the profile asks
//! for that (`CACHEDIR.TAG`, `exclude_if_present`), the folders are found before rsync runs
//! and listed as ordinary rules, so the file still says exactly what was left out.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flux_core::filter::Filter;
use flux_core::profile::Sources;
use flux_core::sync::Locked;

const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Escape rsync wildcard characters (only meaningful in patterns that contain a wildcard).
pub fn escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn expand_home(body: &str, home: Option<&Path>) -> String {
    match (body.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", h.display().to_string().trim_end_matches('/'))
        }
        _ => body.to_owned(),
    }
}

/// One gitignore-style pattern as rsync rules (usually one; two when `/**/` must also
/// match zero folders).
pub fn translate(pattern: &str, home: Option<&Path>) -> Vec<String> {
    let pattern = pattern.trim();
    let (sign, body) = match pattern.strip_prefix('!') {
        Some(rest) => ('+', rest),
        None => ('-', pattern),
    };
    let body = expand_home(body, home);
    if body.is_empty() || body.starts_with('#') {
        return Vec::new();
    }
    let (body, dir_suffix) = match body.strip_suffix('/') {
        Some(b) => (b.to_owned(), "/"),
        None => (body.clone(), ""),
    };
    if body.is_empty() {
        return Vec::new();
    }
    let pat = if let Some(rest) = body.strip_prefix("**/") {
        // Any depth: rsync matches unanchored patterns at the end of the path.
        rest.to_owned()
    } else if body.starts_with('/') {
        body
    } else if body.contains('/') {
        // gitignore anchors a pattern with an inner slash to its base (here: `/`).
        format!("/{body}")
    } else {
        body
    };
    let mut out = vec![format!("{sign} {pat}{dir_suffix}")];
    if pat.contains("/**/") {
        out.push(format!("{sign} {}{dir_suffix}", pat.replace("/**/", "/")));
    }
    out
}

/// Rules for a profile's own exclusions, first-match-wins.
pub fn profile_rules(sources: &Sources, home: Option<&Path>) -> Vec<String> {
    sources
        .exclude
        .iter()
        .rev()
        .flat_map(|p| translate(p, home))
        .collect()
}

/// The rules file for one backup.
pub struct RuleSet {
    /// Always left out (the backup repositories themselves).
    pub never: Vec<PathBuf>,
    /// Folders whose contents are skipped because of a marker file.
    pub marked: Vec<PathBuf>,
    /// The profile's exclusions, translated.
    pub profile: Vec<String>,
}

impl RuleSet {
    pub fn new(sources: &Sources, home: Option<&Path>, never: Vec<PathBuf>) -> Self {
        Self {
            never,
            marked: Vec::new(),
            profile: profile_rules(sources, home),
        }
    }

    /// The file's text, with comments explaining each group.
    pub fn render(&self, heading: &str) -> String {
        let mut s = String::new();
        s.push_str(&format!("# {heading}\n"));
        s.push_str(
            "# rsync filter rules (used with --exclude-from). The first rule that matches a\n\
             # path decides: \"- \" leaves it out, \"+ \" keeps it.\n",
        );
        if !self.never.is_empty() {
            s.push_str("\n# The backup folders themselves:\n");
            for p in &self.never {
                if let Some(p) = rule_path(p) {
                    s.push_str(&format!("- {}/***\n", escape(&p)));
                }
            }
        }
        if !self.marked.is_empty() {
            s.push_str("\n# Folders marked as caches (CACHEDIR.TAG) or with a no-backup marker file,\n# found when this backup started:\n");
            for p in &self.marked {
                if let Some(p) = rule_path(p) {
                    s.push_str(&format!("- {}/*\n", escape(&p)));
                }
            }
        }
        if !self.profile.is_empty() {
            s.push_str("\n# Exclusions from the profile:\n");
            for r in &self.profile {
                s.push_str(r);
                s.push('\n');
            }
        }
        s
    }
}

/// A path usable in a line-based rules file (no newlines), without a trailing slash.
fn rule_path(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    (!s.contains(['\n', '\r'])).then(|| s.trim_end_matches('/').to_owned())
}

fn is_cachedir_tag(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; CACHEDIR_SIGNATURE.len()];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
        && buf == CACHEDIR_SIGNATURE
}

/// Folders below `sources` whose contents the profile skips because of a marker file
/// (`CACHEDIR.TAG` when `exclude_caches`, or a name in `exclude_if_present`). Excluded
/// folders and `never` aren't searched. Sorted.
pub fn find_marked(sources: &Sources, filter: &Filter, never: &[PathBuf]) -> Vec<PathBuf> {
    if !filter.has_markers() {
        return Vec::new();
    }
    let roots = flux_fs::scan::normalize_sources(&sources.paths);
    let Some((first, rest)) = roots.split_first() else {
        return Vec::new();
    };
    let mut b = ignore::WalkBuilder::new(first);
    for r in rest {
        b.add(r);
    }
    b.standard_filters(false)
        .follow_links(false)
        .same_file_system(sources.one_file_system);
    let filter = filter.clone();
    let never = never.to_vec();
    b.filter_entry(move |e| {
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        !never.iter().any(|n| e.path() == n) && !filter.is_excluded(e.path(), is_dir)
    });
    let found = Mutex::new(Vec::new());
    let exclude_caches = sources.exclude_caches;
    let markers = &sources.exclude_if_present;
    b.build_parallel().run(|| {
        let found = &found;
        Box::new(move |e| {
            if let Ok(e) = e
                && e.depth() > 0
                && e.file_type().is_some_and(|t| !t.is_dir())
            {
                let name = e.file_name().as_encoded_bytes();
                let hit = (exclude_caches && name == b"CACHEDIR.TAG" && is_cachedir_tag(e.path()))
                    || markers.iter().any(|m| m.as_bytes() == name);
                if hit && let Some(parent) = e.path().parent() {
                    found.locked().push(parent.to_owned());
                }
            }
            ignore::WalkState::Continue
        })
    });
    let mut v = found.into_value();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(p: &str) -> Vec<String> {
        translate(p, Some(Path::new("/home/me")))
    }

    #[test]
    fn translates_gitignore_patterns() {
        assert_eq!(t("*.tmp"), ["- *.tmp"]);
        assert_eq!(t("node_modules/"), ["- node_modules/"]);
        assert_eq!(t("/home/me/Downloads"), ["- /home/me/Downloads"]);
        assert_eq!(t("~/Videos/"), ["- /home/me/Videos/"]);
        assert_eq!(t("~"), ["- /home/me"]);
        assert_eq!(t("build/out"), ["- /build/out"]);
        assert_eq!(t("**/build/out"), ["- build/out"]);
        assert_eq!(t("**/.git"), ["- .git"]);
        assert_eq!(t("!keep.tmp"), ["+ keep.tmp"]);
        assert_eq!(t("/a/**/b"), ["- /a/**/b", "- /a/b"]);
        assert!(t("  ").is_empty());
        assert!(t("# comment").is_empty());
        assert!(t("/").is_empty());
    }

    #[test]
    fn reverses_for_first_match_wins() {
        let s = Sources {
            paths: vec!["/home/me".into()],
            exclude: vec!["*.tmp".into(), "!keep.tmp".into()],
            exclude_larger_than: None,
            exclude_caches: false,
            exclude_if_present: vec![],
            one_file_system: false,
            exclude_gitignored: false,
            volumes: Vec::new(),
        };
        assert_eq!(profile_rules(&s, None), ["+ keep.tmp", "- *.tmp"]);
    }

    #[test]
    fn escapes_and_renders() {
        assert_eq!(escape("/a/[x]*?\\"), "/a/\\[x]\\*\\?\\\\");
        let r = RuleSet {
            never: vec!["/mnt/b/flux-rsync".into()],
            marked: vec!["/home/me/.cache".into(), "/bad\nname".into()],
            profile: vec!["- *.tmp".into()],
        };
        let text = r.render("test");
        assert!(text.contains("- /mnt/b/flux-rsync/***\n"));
        assert!(text.contains("- /home/me/.cache/*\n"));
        assert!(!text.contains("bad"));
        assert!(text.ends_with("- *.tmp\n"));
    }

    #[test]
    fn finds_marked_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["a/cache", "a/fake", "a/skip", "a/excluded/cache", "a/plain"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("a/cache/CACHEDIR.TAG"), CACHEDIR_SIGNATURE).unwrap();
        std::fs::write(root.join("a/fake/CACHEDIR.TAG"), b"not a tag").unwrap();
        std::fs::write(root.join("a/skip/.nobackup"), b"").unwrap();
        std::fs::write(
            root.join("a/excluded/cache/CACHEDIR.TAG"),
            CACHEDIR_SIGNATURE,
        )
        .unwrap();
        let sources = Sources {
            paths: vec![root.join("a")],
            exclude: vec!["excluded/".into()],
            exclude_larger_than: None,
            exclude_caches: true,
            exclude_if_present: vec![".nobackup".into()],
            one_file_system: false,
            exclude_gitignored: false,
            volumes: Vec::new(),
        };
        let filter = Filter::new(&sources, None).unwrap();
        let found = find_marked(&sources, &filter, &[]);
        assert_eq!(found, [root.join("a/cache"), root.join("a/skip")]);
    }
}
