//! Parallel scan of the backup sources into a sorted list of [`Entry`]s.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

use flux_core::filter::Filter;
use flux_core::profile::SymlinkMode;
use flux_core::progress::{Event, Progress, Reporter};

use crate::entry::{Entry, Kind};
use crate::fsops::{XattrPolicy, read_xattrs};
use crate::snapshot::FileError;
use flux_core::sync::Locked;

pub struct ScanConfig<'a> {
    pub sources: &'a [PathBuf],
    pub filter: &'a Filter,
    pub one_file_system: bool,
    pub symlinks: SymlinkMode,
    pub xattrs: XattrPolicy,
    /// Hash every regular file (checksum change detection).
    pub hash: bool,
    /// Only count (`ScanResult::counts`): keep no entries (a size estimate needn't hold a
    /// list of every file in memory).
    pub count_only: bool,
    pub progress: &'a Progress,
    pub report: Reporter<'a>,
}

pub struct ScanResult {
    /// Sorted by path, no duplicates. Empty with [`ScanConfig::count_only`].
    pub entries: Vec<Entry>,
    /// What was found: files (and other non-folders), folders, and the files' bytes.
    pub counts: ScanCounts,
    pub errors: Vec<FileError>,
    /// Sources that did not exist.
    pub missing_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanCounts {
    /// Regular files, links and other non-folders.
    pub files: u64,
    pub folders: u64,
    /// Bytes in regular files.
    pub bytes: u64,
}

impl ScanCounts {
    fn add(&mut self, e: &Entry) {
        match e.kind {
            Kind::File => {
                self.files += 1;
                self.bytes += e.size;
            }
            Kind::Dir => self.folders += 1,
            _ => self.files += 1,
        }
    }
}

/// Drop sources that lie inside other sources, and sort.
pub fn normalize_sources(sources: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = sources.iter().map(|p| lexical_normalize(p)).collect();
    v.sort();
    v.dedup();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in v {
        if !out.iter().any(|o| p.starts_with(o)) {
            out.push(p);
        }
    }
    out
}

fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    out
}

struct Ctx<'a> {
    cfg: &'a ScanConfig<'a>,
    out: Mutex<Vec<Entry>>,
    /// With `count_only`: what was found, instead of `out`.
    counts: Mutex<ScanCounts>,
    errors: Mutex<Vec<FileError>>,
    /// (dev, ino) of walked directories; only tracked when links may be followed, to stop
    /// a link to an ancestor from looping forever.
    visited: Mutex<HashSet<(u64, u64)>>,
    /// How files are opened to be read (for their checksums).
    opener: crate::fsops::SourceOpener,
}

impl Ctx<'_> {
    /// Keep `entries` (or, counting only, just count them).
    fn keep(&self, entries: &mut Vec<Entry>) {
        if self.cfg.count_only {
            let mut c = ScanCounts::default();
            entries.iter().for_each(|e| c.add(e));
            entries.clear();
            let mut total = self.counts.locked();
            total.files += c.files;
            total.folders += c.folders;
            total.bytes += c.bytes;
        } else {
            self.out.locked().append(entries);
        }
    }

    /// Record a directory as walked. Returns false if it was already walked.
    fn visit(&self, m: &fs::Metadata) -> bool {
        self.cfg.symlinks == SymlinkMode::Preserve
            || self.visited.locked().insert((m.dev(), m.ino()))
    }

    fn warn(&self, path: &Path, err: impl std::fmt::Display) {
        let fe = FileError {
            path: path.display().to_string(),
            message: err.to_string(),
        };
        (self.cfg.report)(Event::Warning {
            path: fe.path.clone(),
            message: fe.message.clone(),
        });
        self.cfg.progress.warnings.fetch_add(1, Relaxed);
        self.errors.locked().push(fe);
    }
}

pub fn scan(cfg: &ScanConfig<'_>) -> ScanResult {
    let ctx = Ctx {
        cfg,
        out: Mutex::new(Vec::new()),
        counts: Mutex::new(ScanCounts::default()),
        errors: Mutex::new(Vec::new()),
        visited: Mutex::new(HashSet::new()),
        opener: crate::fsops::SourceOpener::new(
            &normalize_sources(cfg.sources),
            cfg.symlinks != SymlinkMode::Preserve,
        ),
    };
    let sources = normalize_sources(cfg.sources);
    let mut missing = Vec::new();

    // Ancestor directories of each source ("/home", "/home/me" for "/home/me/Docs"), so
    // the data tree has them with the right metadata.
    let mut ancestors = BTreeSet::new();
    for s in &sources {
        let mut a = s.parent();
        while let Some(p) = a {
            if p != Path::new("/") {
                ancestors.insert(p.to_owned());
            }
            a = p.parent();
        }
    }
    for a in &ancestors {
        if sources.iter().any(|s| a.starts_with(s)) {
            continue;
        }
        if let Ok(m) = fs::metadata(a)
            && let Some(e) = make_entry(&ctx, a, &m)
        {
            ctx.keep(&mut vec![e]);
        }
    }

    rayon::scope(|scope| {
        for src in &sources {
            // Follow a symlinked source root: the user means what it points to.
            let meta = match fs::metadata(src) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    (cfg.report)(Event::Warning {
                        path: src.display().to_string(),
                        message: "source does not exist; skipped".into(),
                    });
                    missing.push(src.clone());
                    continue;
                }
                Err(e) => {
                    ctx.warn(src, e);
                    continue;
                }
            };
            if cfg.filter.is_excluded(src, meta.is_dir()) {
                continue;
            }
            let Some(entry) = make_entry(&ctx, src, &meta) else {
                continue;
            };
            let is_dir = entry.kind == Kind::Dir;
            ctx.keep(&mut vec![entry]);
            if is_dir && ctx.visit(&meta) {
                let ctx = &ctx;
                let root_dev = meta.dev();
                scope.spawn(move |s| walk_dir(s, ctx, src.clone(), src, root_dev));
            }
        }
    });

    let mut entries = ctx.out.into_value();
    entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    let counts = if cfg.count_only {
        ctx.counts.into_value()
    } else {
        let mut c = ScanCounts::default();
        entries.iter().for_each(|e| c.add(e));
        c
    };
    ScanResult {
        entries,
        counts,
        errors: ctx.errors.into_value(),
        missing_sources: missing,
    }
}

fn walk_dir<'s>(
    scope: &rayon::Scope<'s>,
    ctx: &'s Ctx<'s>,
    dir: PathBuf,
    root: &'s Path,
    root_dev: u64,
) {
    let rd = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => return ctx.warn(&dir, e),
    };
    let mut children = Vec::new();
    for ent in rd {
        match ent {
            Ok(ent) => children.push(ent),
            Err(e) => ctx.warn(&dir, e),
        }
    }
    let cfg = ctx.cfg;
    if cfg.filter.has_markers() {
        let names: Vec<_> = children.iter().map(|c| c.file_name()).collect();
        if cfg
            .filter
            .skip_dir_by_marker(&dir, names.iter().map(|n| n.as_bytes()))
        {
            return;
        }
    }

    let mut local = Vec::with_capacity(children.len());
    for ent in children {
        if cfg.progress.is_cancelled() {
            return;
        }
        let path = ent.path();
        let meta = match ent.metadata() {
            Ok(m) => m,
            Err(e) => {
                ctx.warn(&path, e);
                continue;
            }
        };
        let (meta, followed) = match maybe_follow(cfg.symlinks, &path, meta, root) {
            Ok(x) => x,
            Err(e) => {
                ctx.warn(&path, e);
                continue;
            }
        };
        let is_dir = meta.is_dir();
        if cfg.filter.is_excluded(&path, is_dir) {
            continue;
        }
        if meta.is_file() && cfg.filter.is_too_large(meta.len()) {
            continue;
        }
        let Some(entry) = make_entry(ctx, &path, &meta) else {
            continue;
        };
        local.push(entry);
        // Don't descend into other filesystems (the mount point itself is kept). A directory
        // reached through a followed link is walked only if not walked before (no loops).
        let same_fs = !cfg.one_file_system || meta.dev() == root_dev;
        if is_dir && same_fs && (ctx.visit(&meta) || !followed) {
            scope.spawn(move |s| walk_dir(s, ctx, path, root, root_dev));
        }
    }
    // One update per directory, not per entry (many threads, one counter).
    cfg.progress.scanned.fetch_add(local.len() as u64, Relaxed);
    ctx.keep(&mut local);
}

/// Apply the symlink policy. Returns the metadata to record and whether a link was followed.
fn maybe_follow(
    mode: SymlinkMode,
    path: &Path,
    meta: fs::Metadata,
    root: &Path,
) -> std::io::Result<(fs::Metadata, bool)> {
    if !meta.file_type().is_symlink() || mode == SymlinkMode::Preserve {
        return Ok((meta, false));
    }
    if mode == SymlinkMode::FollowUnsafe {
        let target = fs::read_link(path)?;
        let resolved = lexical_normalize(&path.parent().unwrap_or(Path::new("/")).join(target));
        if resolved.starts_with(root) {
            return Ok((meta, false));
        }
    }
    match fs::metadata(path) {
        Ok(m) => Ok((m, true)),
        // Dangling link: keep it as a link.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((meta, false)),
        Err(e) => Err(e),
    }
}

fn make_entry(ctx: &Ctx<'_>, path: &Path, m: &fs::Metadata) -> Option<Entry> {
    let cfg = ctx.cfg;
    let kind = Kind::from_file_type(m.file_type());
    let rel = path.as_os_str().as_bytes();
    let rel = rel.strip_prefix(b"/").unwrap_or(rel).to_vec();
    let link = if kind == Kind::Symlink {
        match fs::read_link(path) {
            Ok(t) => Some(t.into_os_string().into_encoded_bytes()),
            Err(e) => {
                ctx.warn(path, e);
                return None;
            }
        }
    } else {
        None
    };
    let xattrs = if kind == Kind::Symlink {
        Vec::new()
    } else {
        read_xattrs(path, cfg.xattrs).unwrap_or_else(|e| {
            ctx.warn(path, format!("cannot read extended attributes: {e}"));
            Vec::new()
        })
    };
    let hash = if cfg.hash && kind == Kind::File {
        match ctx.opener.open(path).and_then(crate::fsops::hash_open) {
            Ok(h) => Some(h),
            Err(e) => {
                ctx.warn(path, e);
                return None;
            }
        }
    } else {
        None
    };
    Some(Entry {
        path: rel,
        kind,
        mode: m.mode() & 0o7777,
        uid: m.uid(),
        gid: m.gid(),
        size: if kind == Kind::File { m.len() } else { 0 },
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec() as u32,
        dev: m.dev(),
        ino: m.ino(),
        nlink: m.nlink(),
        rdev: m.rdev(),
        link,
        hash,
        xattrs,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileScanError {
    #[error(transparent)]
    Filter(#[from] flux_core::filter::FilterError),
    #[error("none of the source paths exist")]
    NoSources,
    #[error("cancelled")]
    Cancelled,
    #[error("thread pool: {0}")]
    Pool(String),
}

/// Worker pool sized by the profile's `threads` option (0 = automatic, at most 16).
pub fn thread_pool(profile: &flux_core::Profile) -> Result<rayon::ThreadPool, ProfileScanError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(match profile.options.threads {
            0 => std::thread::available_parallelism().map_or(4, |n| n.get().min(16)),
            n => n as usize,
        })
        .build()
        .map_err(|e| ProfileScanError::Pool(e.to_string()))
}

pub fn xattr_policy(profile: &flux_core::Profile) -> XattrPolicy {
    XattrPolicy {
        acl: profile.options.preserve_acl,
        other: profile.options.preserve_xattr,
    }
}

/// What an exact restore leaves alone because the profile's backups never take it: its
/// exclusions, size limit and cache markers, and a local backup drive inside the sources.
pub fn exact_filter(
    profile: &flux_core::Profile,
) -> Result<Filter, flux_core::filter::FilterError> {
    let mut f = Filter::new(&profile.sources, flux_core::sys::home_dir().as_deref())?;
    if let flux_core::profile::Target::Local { path, .. } = &profile.target
        && !path.as_os_str().is_empty()
    {
        f.add_hard_exclude(path);
    }
    Ok(f)
}

/// Scan a profile's sources with its filters, symlink and xattr rules. `never` lists
/// paths that are always skipped (the backup repository itself). Run inside the pool from
/// [`thread_pool`] to control parallelism.
pub fn scan_profile(
    profile: &flux_core::Profile,
    never: &[PathBuf],
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<ScanResult, ProfileScanError> {
    scan_profile_with(profile, never, false, progress, report)
}

/// [`scan_profile`] that only counts (`ScanResult::counts`), keeping no entries.
pub fn count_profile(
    profile: &flux_core::Profile,
    never: &[PathBuf],
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<ScanResult, ProfileScanError> {
    scan_profile_with(profile, never, true, progress, report)
}

fn scan_profile_with(
    profile: &flux_core::Profile,
    never: &[PathBuf],
    count_only: bool,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<ScanResult, ProfileScanError> {
    let mut filter = Filter::new(&profile.sources, flux_core::sys::home_dir().as_deref())?;
    for n in never {
        filter.add_hard_exclude(n);
    }
    let opts = &profile.options;
    let scanned = scan(&ScanConfig {
        sources: &profile.sources.paths,
        filter: &filter,
        one_file_system: profile.sources.one_file_system,
        symlinks: opts.symlinks,
        xattrs: xattr_policy(profile),
        hash: !count_only && opts.change_detection == flux_core::profile::ChangeDetection::Checksum,
        count_only,
        progress,
        report,
    });
    if progress.is_cancelled() {
        return Err(ProfileScanError::Cancelled);
    }
    if scanned.missing_sources.len() == normalize_sources(&profile.sources.paths).len() {
        return Err(ProfileScanError::NoSources);
    }
    Ok(scanned)
}

/// Merge into `entries` (a scan, sorted by path) the entries of `previous` (the last
/// snapshot's, in any order) that lie in one of `kept` (sources whose drive isn't
/// connected), and their parent folders the scan lacks. The new snapshot then holds those
/// sources as they were, rather than as deleted. Scanned entries win over kept ones.
/// Returns how many entries were kept.
pub fn carry_over<I>(entries: &mut Vec<Entry>, previous: I, kept: &[PathBuf]) -> usize
where
    I: IntoIterator<Item = Entry>,
{
    use std::os::unix::ffi::OsStrExt;
    if kept.is_empty() {
        return 0;
    }
    let prefixes: Vec<Vec<u8>> = kept
        .iter()
        .map(|p| {
            p.as_os_str()
                .as_bytes()
                .trim_ascii_start()
                .strip_prefix(b"/")
                .unwrap_or(p.as_os_str().as_bytes())
                .to_vec()
        })
        .collect();
    let inside = |path: &[u8]| {
        prefixes.iter().any(|p| {
            path == p.as_slice()
                || (path.starts_with(p) && path.get(p.len()) == Some(&b'/'))
                // a parent folder of a kept source
                || (p.starts_with(path) && p.get(path.len()) == Some(&b'/'))
        })
    };
    let before = entries.len();
    let have: std::collections::HashSet<Vec<u8>> = entries.iter().map(|e| e.path.clone()).collect();
    entries.extend(
        previous
            .into_iter()
            .filter(|e| inside(&e.path) && !have.contains(&e.path)),
    );
    let added = entries.len() - before;
    if added > 0 {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
    }
    added
}

#[cfg(test)]
mod tests {

    #[test]
    fn counting_only_matches_a_full_scan() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("a/one.txt"), "12345").unwrap();
        fs::write(src.join("a/b/two.txt"), "123").unwrap();
        std::os::unix::fs::symlink("one.txt", src.join("a/link")).unwrap();
        let profile = flux_core::Profile::new(
            "t",
            flux_core::profile::Target::Local {
                path: dir.path().join("tgt"),
                fs_uuid: None,
            },
            vec![src.clone()],
        );
        let p = Progress::new();
        let full = scan_profile(&profile, &[], &p, &flux_core::progress::silent).unwrap();
        let counted = count_profile(&profile, &[], &p, &flux_core::progress::silent).unwrap();
        assert!(counted.entries.is_empty());
        assert_eq!(counted.counts, full.counts);
        assert_eq!((full.counts.files, full.counts.bytes), (3, 8));
    }
    use super::*;

    fn entry(path: &str, size: u64) -> Entry {
        Entry {
            path: path.as_bytes().to_vec(),
            kind: if size == 0 {
                crate::entry::Kind::Dir
            } else {
                crate::entry::Kind::File
            },
            mode: 0o755,
            uid: 0,
            gid: 0,
            size,
            mtime: 1,
            mtime_nsec: 0,
            dev: 1,
            ino: size,
            nlink: 1,
            rdev: 0,
            link: None,
            hash: None,
            xattrs: Vec::new(),
        }
    }

    #[test]
    fn carry_over_keeps_missing_sources() {
        // Scanned now: the home folder. The photos drive isn't there.
        let mut now = vec![
            entry("home", 0),
            entry("home/me", 0),
            entry("home/me/new.txt", 7),
        ];
        let previous = vec![
            entry("home", 0),
            entry("home/me", 0),
            entry("home/me/old.txt", 3), // deleted since: stays deleted
            entry("mnt", 0),
            entry("mnt/photos", 0),
            entry("mnt/photos/a.jpg", 10),
            entry("mnt/photos/2024/b.jpg", 11),
            entry("mnt/photosynth.txt", 4), // a neighbour, not inside the source
        ];
        let kept = carry_over(&mut now, previous, &[PathBuf::from("/mnt/photos")]);
        let paths: Vec<&str> = now
            .iter()
            .map(|e| std::str::from_utf8(&e.path).unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "home",
                "home/me",
                "home/me/new.txt",
                "mnt",
                "mnt/photos",
                "mnt/photos/2024/b.jpg",
                "mnt/photos/a.jpg",
            ]
        );
        assert_eq!(kept, 4);
        assert_eq!(carry_over(&mut now, Vec::new(), &[]), 0);
    }

    #[test]
    fn normalizes_sources() {
        let v = normalize_sources(&[
            "/home/me/Docs".into(),
            "/home/me".into(),
            "/etc/../etc".into(),
            "/home/me2".into(),
        ]);
        assert_eq!(
            v,
            vec![PathBuf::from("/etc"), "/home/me".into(), "/home/me2".into()]
        );
    }
}
