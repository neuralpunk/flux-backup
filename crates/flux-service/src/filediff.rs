//! Comparing two versions of a file: from two snapshots, or a snapshot and the file on
//! disk now. Text files get a line-by-line comparison to show in the app; any file can be
//! opened in a comparison program (Meld and the like), from local copies.

use std::path::{Path, PathBuf};

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

use flux_core::Scope;

use crate::{ApiError, Result, Service};

/// Larger files aren't compared line by line (a comparison program still can).
const TEXT_MAX: u64 = 8 << 20;
/// Lines shown at most; the rest is counted.
const LINES_MAX: usize = 5000;
/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;

/// Comparison programs Flux looks for, best first: (command, name).
pub const TOOLS: &[(&str, &str)] = &[
    ("meld", "Meld"),
    ("kompare", "Kompare"),
    ("kdiff3", "KDiff3"),
    ("diffuse", "Diffuse"),
];

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DiffLine {
    /// `same`, `removed`, `added`, or `skip` (unchanged lines left out; `text` is empty and
    /// `count` says how many).
    pub kind: &'static str,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct FileDiff {
    /// Local copies of both sides (the file itself for "now"), for a comparison program.
    pub old_file: String,
    pub new_file: String,
    pub old_size: u64,
    pub new_size: u64,
    pub identical: bool,
    /// Not text (or too large to compare here): only sizes and sameness are known.
    pub binary: bool,
    pub too_large: bool,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
    /// Lines left out after [`LINES_MAX`].
    pub truncated: usize,
}

/// Compare `path` in snapshot `old` with `new` (another snapshot, or `None`: the file on
/// disk now).
pub fn diff_file_impl(
    state: &Service,
    id: String,
    path: String,
    old: String,
    new: Option<String>,
) -> Result<FileDiff> {
    if state.scope() == Scope::System {
        return Err(ApiError::new(
            "error",
            "Files from system backups can't be compared here (they're root's); restore them to a folder first.",
        ));
    }
    let old_file = crate::extras::copy_out(state, &id, &old, &path)?;
    let new_file = match &new {
        Some(snap) => crate::extras::copy_out(state, &id, snap, &path)?,
        None => {
            let p = PathBuf::from(&path);
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.is_file() => p,
                Ok(_) => {
                    return Err(ApiError::new(
                        "error",
                        format!("{path} isn't a file now, so there's nothing to compare it with."),
                    ));
                }
                Err(_) => {
                    return Err(ApiError::new(
                        "error",
                        format!("{path} isn't on your computer now."),
                    ));
                }
            }
        }
    };
    compare(&old_file, &new_file)
}

/// Compare two local files.
pub fn compare(old_file: &Path, new_file: &Path) -> Result<FileDiff> {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let (old_size, new_size) = (size(old_file), size(new_file));
    let mut out = FileDiff {
        old_file: old_file.display().to_string(),
        new_file: new_file.display().to_string(),
        old_size,
        new_size,
        identical: false,
        binary: false,
        too_large: old_size > TEXT_MAX || new_size > TEXT_MAX,
        added: 0,
        removed: 0,
        lines: Vec::new(),
        truncated: 0,
    };
    if out.too_large {
        out.identical = old_size == new_size && same_bytes(old_file, new_file);
        return Ok(out);
    }
    let read = |p: &Path| {
        std::fs::read(p).map_err(|e| ApiError::new("error", format!("{}: {e}", p.display())))
    };
    let (a, b) = (read(old_file)?, read(new_file)?);
    out.identical = a == b;
    let (Some(a), Some(b)) = (as_text(&a), as_text(&b)) else {
        out.binary = true;
        return Ok(out);
    };
    if out.identical {
        return Ok(out);
    }
    let diff = TextDiff::from_lines(a, b);
    let groups = diff.grouped_ops(CONTEXT);
    let mut prev_end = 0;
    for group in &groups {
        // Unchanged lines between (and before) the groups, as one "skip" line.
        let first = group.first().map_or(0, |op| op.old_range().start);
        let skipped = first.saturating_sub(prev_end);
        prev_end = group.last().map_or(prev_end, |op| op.old_range().end);
        if skipped > 0 {
            push(
                &mut out,
                DiffLine {
                    kind: "skip",
                    old: None,
                    new: None,
                    text: String::new(),
                    count: skipped,
                },
            );
        }
        for op in group {
            for c in diff.iter_changes(op) {
                let kind = match c.tag() {
                    ChangeTag::Equal => "same",
                    ChangeTag::Delete => {
                        out.removed += 1;
                        "removed"
                    }
                    ChangeTag::Insert => {
                        out.added += 1;
                        "added"
                    }
                };
                push(
                    &mut out,
                    DiffLine {
                        kind,
                        old: c.old_index().map(|i| i + 1),
                        new: c.new_index().map(|i| i + 1),
                        text: c.value().trim_end_matches(['\n', '\r']).to_owned(),
                        count: 1,
                    },
                );
            }
        }
    }
    // Unchanged lines after the last change.
    let last_end = prev_end;
    let total = diff.old_len();
    if total > last_end {
        push(
            &mut out,
            DiffLine {
                kind: "skip",
                old: None,
                new: None,
                text: String::new(),
                count: total - last_end,
            },
        );
    }
    Ok(out)
}

/// The file as text, if it is (no NUL byte near the start, valid UTF-8).
fn as_text(v: &[u8]) -> Option<&str> {
    if v[..v.len().min(8192)].contains(&0) {
        return None;
    }
    std::str::from_utf8(v).ok()
}

fn push(out: &mut FileDiff, line: DiffLine) {
    if out.lines.len() < LINES_MAX {
        out.lines.push(line);
    } else if line.kind != "skip" {
        out.truncated += 1;
    }
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(na), Ok(nb)) = (fa.read(&mut ba), fb.read(&mut bb)) else {
            return false;
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// The comparison program to offer: the one chosen in `settings.toml` (`diff_tool`), or the
/// first installed of [`TOOLS`]. `(command, name)`.
pub fn diff_tool(state: &Service) -> Option<(String, String)> {
    if let Some(cmd) = flux_runner::prefs::Settings::load(&state.paths).diff_tool {
        let cmd = cmd.trim().to_owned();
        if cmd.is_empty() {
            return None; // "none": the built-in view only
        }
        let name = TOOLS
            .iter()
            .find(|(c, _)| cmd.split_whitespace().next() == Some(c))
            .map_or_else(|| cmd.clone(), |(_, n)| (*n).to_owned());
        return Some((cmd, name));
    }
    installed_tools()
        .into_iter()
        .next()
        .map(|(c, n)| (c.to_owned(), n.to_owned()))
}

/// The known comparison programs found on `PATH`.
pub fn installed_tools() -> Vec<(&'static str, &'static str)> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut out: Vec<(&str, &str)> = Vec::new();
    for &(cmd, name) in TOOLS {
        if out.iter().any(|(c, _)| *c == cmd) {
            continue;
        }
        if std::env::split_paths(&path).any(|d| is_executable(&d.join(cmd))) {
            out.push((cmd, name));
        }
    }
    out
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(a: &str, b: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let (pa, pb) = (t.path().join("a"), t.path().join("b"));
        std::fs::write(&pa, a).unwrap();
        std::fs::write(&pb, b).unwrap();
        (t, pa, pb)
    }

    #[test]
    fn text_changes_with_context() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 10\n", "line ten\n") + "line 21\n";
        let (_t, a, b) = files(&old, &new);
        let d = compare(&a, &b).unwrap();
        assert!(!d.identical && !d.binary);
        assert_eq!((d.added, d.removed), (2, 1));
        let kinds: Vec<&str> = d.lines.iter().map(|l| l.kind).collect();
        // 6 unchanged lines skipped, 3 of context, the change, 3 of context, 6 skipped,
        // 3 of context, the added line.
        assert_eq!(kinds[0], "skip");
        assert_eq!(d.lines[0].count, 6);
        let removed = d.lines.iter().find(|l| l.kind == "removed").unwrap();
        assert_eq!(
            (removed.old, removed.new, removed.text.as_str()),
            (Some(10), None, "line 10")
        );
        let added: Vec<&str> = d
            .lines
            .iter()
            .filter(|l| l.kind == "added")
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(added, ["line ten", "line 21"]);
        assert!(
            d.lines
                .iter()
                .any(|l| l.kind == "skip" && l.count == 6 && l.old.is_none())
        );
        assert_ne!(
            d.lines.last().unwrap().kind,
            "skip",
            "the change is at the end"
        );
    }

    #[test]
    fn identical_binary_and_trailing_skip() {
        let (_t, a, b) = files("same\n", "same\n");
        let d = compare(&a, &b).unwrap();
        assert!(d.identical && d.lines.is_empty());

        let (_t, a, b) = files("a\0b", "a\0c");
        let d = compare(&a, &b).unwrap();
        assert!(d.binary && !d.identical);

        let old: String = (1..=10).map(|i| format!("{i}\n")).collect();
        let new = old.replacen("1\n", "one\n", 1);
        let (_t, a, b) = files(&old, &new);
        let d = compare(&a, &b).unwrap();
        let last = d.lines.last().unwrap();
        assert_eq!((last.kind, last.count), ("skip", 6), "{:?}", d.lines);
    }
}
