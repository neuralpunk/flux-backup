//! Finding files: search every snapshot by name, the versions of one file, and what a
//! restore would change before it does.

use std::path::PathBuf;

use serde::Serialize;

use flux_fs::fsops::XattrPolicy;
use flux_fs::history::{self, Found, Version};
use flux_fs::restore::{self, RestorePreview, RestoreRequest};
use flux_fs::safe::LinkPolicy;
use flux_fs::snapshot::SnapshotMeta;

use crate::{ApiError, Result, Service};

/// At most this many results are returned; the total is reported.
const MAX_HITS: usize = 500;

#[derive(Debug, Serialize)]
pub struct FindResult {
    pub hits: Vec<Hit>,
    /// Matching paths in all, when more than `hits`.
    pub total: usize,
    /// Snapshots searched.
    pub snapshots: usize,
}

/// A path found in the snapshots, and whether it's on the computer now.
#[derive(Debug, Serialize)]
pub struct Hit {
    #[serde(flatten)]
    pub found: Found,
    /// Checked on every search, so something restored since isn't shown as deleted.
    pub on_disk: Option<bool>,
}

type Matcher = Box<dyn Fn(&[u8]) -> bool + Sync>;

/// The part of a path a query is matched against: the name, or the whole path.
fn part(path: &[u8], full_path: bool) -> &[u8] {
    if full_path {
        path
    } else {
        path.rsplit(|&b| b == b'/').next().unwrap_or(path)
    }
}

/// How the app's search box matches: plain text is a case-insensitive part of the name
/// (of the path when it has a `/`); `*`, `?` and `[…]` make it a pattern.
fn matcher(query: &str) -> Result<Matcher> {
    let q = query.trim();
    if q.is_empty() {
        return Err(ApiError::new(
            "error",
            "Type part of a file name to search for.",
        ));
    }
    let full_path = q.contains('/');
    if q.contains(['*', '?', '[']) {
        let glob = globset::GlobBuilder::new(q.trim_start_matches('/'))
            .literal_separator(full_path)
            .case_insensitive(true)
            .build()
            .map_err(|e| ApiError::new("error", format!("Not a valid pattern: {e}")))?
            .compile_matcher();
        return Ok(Box::new(move |path: &[u8]| {
            use std::os::unix::ffi::OsStrExt;
            glob.is_match(std::ffi::OsStr::from_bytes(part(path, full_path)))
        }));
    }
    let needle = q.trim_start_matches('/').to_lowercase();
    Ok(Box::new(move |path: &[u8]| {
        let hay = part(path, full_path);
        if needle.is_ascii() {
            hay.windows(needle.len())
                .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
        } else {
            String::from_utf8_lossy(hay)
                .to_lowercase()
                .contains(&needle)
        }
    }))
}

/// Search every snapshot of a profile.
pub fn find_impl(state: &Service, id: String, query: String) -> Result<FindResult> {
    let matches = matcher(&query)?;
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snaps = repo.snapshots()?;
    flux_runner::index::prune(&state.paths, &pid, &snaps);
    let open = |s: &SnapshotMeta| flux_runner::index::entries(&repo, &state.paths, &pid, s);
    let mut hits = history::find(&open, &snaps, &*matches)?;
    let total = hits.len();
    // Files people look for first: those still in the newest snapshot, then by path.
    let newest = snaps.last().map(|s| s.id.as_str());
    hits.sort_by(|a, b| {
        (Some(b.latest.as_str()) == newest)
            .cmp(&(Some(a.latest.as_str()) == newest))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(MAX_HITS);
    let hits = hits
        .into_iter()
        .map(|found| Hit {
            on_disk: crate::views::on_disk(&found.path),
            found,
        })
        .collect();
    Ok(FindResult {
        hits,
        total,
        snapshots: snaps.len(),
    })
}

/// Every distinct version of one path, oldest first.
pub fn versions_impl(state: &Service, id: String, path: String) -> Result<Vec<Version>> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snaps = repo.snapshots()?;
    let rel = path.trim_start_matches('/').trim_end_matches('/');
    flux_runner::index::prune(&state.paths, &pid, &snaps);
    let open = |s: &SnapshotMeta| flux_runner::index::entries(&repo, &state.paths, &pid, s);
    Ok(history::history(&open, &snaps, rel.as_bytes())?)
}

/// What restoring `paths` would do: new, differing (replaced or kept aside) and identical
/// items. Nothing is changed.
pub fn restore_preview_impl(
    state: &Service,
    id: String,
    snapshot: String,
    paths: Vec<String>,
    to: Option<String>,
    overwrite: bool,
    exact: bool,
) -> Result<RestorePreview> {
    let (pid, p) = state.load(&id)?;
    let repo = state.open(&pid, &p)?;
    let snap = repo.find(&snapshot)?;
    let to = to.map(PathBuf::from);
    if to.as_ref().is_some_and(|t| !t.is_absolute()) {
        return Err(ApiError::new(
            "error",
            "the destination must be an absolute path",
        ));
    }
    let filter = exact
        .then(|| flux_fs::scan::exact_filter(&p))
        .transpose()
        .map_err(|e| ApiError::new("error", e.to_string()))?;
    let req = RestoreRequest {
        snapshot: &snap,
        paths: paths.into_iter().map(PathBuf::from).collect(),
        to,
        keep_existing: p.options.backup_before_restore && !overwrite,
        dry_run: true,
        xattrs: XattrPolicy {
            acl: false,
            other: false,
        },
        links: LinkPolicy::for_process(),
        exact: filter.as_ref(),
    };
    let entries = repo
        .entries(&snap)?
        .map(|e| e.map_err(|e| std::io::Error::other(e.to_string())));
    restore::preview(entries, &req).map_err(|e| ApiError::new("error", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_box_matching() {
        let m = matcher("REPORT").unwrap();
        assert!(m(b"home/me/Annual-report.odt"));
        assert!(!m(b"home/report/notes.txt"), "plain text matches names");
        let m = matcher("report/").unwrap();
        assert!(m(b"home/report/notes.txt"), "a / matches paths");
        let m = matcher("*.ODT").unwrap();
        assert!(m(b"home/me/a.odt"));
        assert!(!m(b"home/me/a.odt.bak"));
        let m = matcher("café").unwrap();
        assert!(m(b"home/me/Caf\xc3\x89 menu.pdf"));
        assert!(matcher("  ").is_err());
    }
}
