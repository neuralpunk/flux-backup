//! What a snapshot contains, read from the snapshot itself: a scan of a local folder, a
//! remote `find`, or (Borg) the tool's own listing. Listings are cached on this machine
//! (snapshots don't change), so browsing a snapshot twice is instant.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use flux_core::filter::Filter;
use flux_core::profile::{Sources, SymlinkMode};
use flux_core::progress::Progress;
use flux_fs::entry::{Entry, Kind};
use flux_fs::fsops::XattrPolicy;
use flux_mirror::manifest::{ManifestReader, ManifestWriter};
use flux_transport::{SshTarget, shell_quote};

/// Entries of a local folder tree, with paths relative to `data`, sorted by path.
pub fn scan_local(data: &Path, xattrs: XattrPolicy) -> io::Result<Vec<Entry>> {
    if !data.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", data.display()),
        ));
    }
    let empty = Sources {
        paths: Vec::new(),
        exclude: Vec::new(),
        exclude_larger_than: None,
        exclude_caches: false,
        exclude_if_present: Vec::new(),
        one_file_system: false,
        exclude_gitignored: false,
        volumes: Vec::new(),
    };
    let filter = Filter::new(&empty, None).map_err(io::Error::other)?;
    let progress = Progress::default();
    let data = flux_fs::scan::normalize_sources(&[data.to_owned()])
        .pop()
        .expect("one path");
    let res = flux_fs::scan::scan(&flux_fs::scan::ScanConfig {
        sources: std::slice::from_ref(&data),
        filter: &filter,
        one_file_system: false,
        symlinks: SymlinkMode::Preserve,
        xattrs,
        hash: false,
        count_only: false,
        progress: &progress,
        report: &|_| {},
    });
    for e in &res.errors {
        tracing::warn!("{}: {}", e.path, e.message);
    }
    let prefix = data.as_os_str().as_bytes();
    let prefix = prefix.strip_prefix(b"/").unwrap_or(prefix);
    Ok(res
        .entries
        .into_iter()
        .filter_map(|mut e| {
            let rest = e.path.strip_prefix(prefix)?.strip_prefix(b"/")?;
            e.path = rest.to_vec();
            Some(e)
        })
        .collect())
}

/// `find -printf` fields, NUL-separated (names may contain anything but NUL).
const FIND_FORMAT: &str = r"%y\0%m\0%U\0%G\0%s\0%T@\0%n\0%i\0%D\0%l\0%P\0";
const FIND_FIELDS: usize = 11;

/// Entries of folder `data` on an SSH server (one `find`). Extended attributes aren't
/// listed.
pub fn scan_remote(ssh: &SshTarget, data: &str) -> io::Result<Vec<Entry>> {
    let cmd = format!(
        "cd {} && find . -mindepth 1 -printf '{FIND_FORMAT}'",
        shell_quote(data)
    );
    let out = ssh
        .command(&cmd)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "listing {data} on {} failed: {}",
            ssh.destination(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    parse_find(&out.stdout)
}

fn parse_find(raw: &[u8]) -> io::Result<Vec<Entry>> {
    let fields: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let bad = || io::Error::other("unexpected output from find");
    let mut out = Vec::with_capacity(fields.len() / FIND_FIELDS);
    for rec in fields.chunks(FIND_FIELDS) {
        if rec.len() < FIND_FIELDS {
            break; // the empty tail after the last NUL
        }
        let s = |i: usize| std::str::from_utf8(rec[i]).map_err(|_| bad());
        let num = |i: usize| s(i)?.parse::<u64>().map_err(|_| bad());
        let kind = match rec[0] {
            b"d" => Kind::Dir,
            b"l" => Kind::Symlink,
            b"p" => Kind::Fifo,
            b"c" => Kind::CharDevice,
            b"b" => Kind::BlockDevice,
            b"s" => Kind::Socket,
            _ => Kind::File,
        };
        let (sec, frac) = s(5)?.split_once('.').unwrap_or((s(5)?, "0"));
        let nsec = format!("{frac:0<9}");
        out.push(Entry {
            path: rec[10].to_vec(),
            kind,
            mode: u32::from_str_radix(s(1)?, 8).map_err(|_| bad())?,
            uid: num(2)? as u32,
            gid: num(3)? as u32,
            size: if kind == Kind::File { num(4)? } else { 0 },
            mtime: sec.parse().map_err(|_| bad())?,
            mtime_nsec: nsec[..9].parse().map_err(|_| bad())?,
            dev: num(8)?,
            ino: num(7)?,
            nlink: num(6)?,
            rdev: 0,
            link: (kind == Kind::Symlink).then(|| rec[9].to_vec()),
            hash: None,
            xattrs: Vec::new(),
        });
    }
    out.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Cached snapshot listings for one repository, in Flux's manifest format.
pub struct ListingCache {
    dir: PathBuf,
}

impl ListingCache {
    /// `key` identifies the repository (e.g. its location).
    pub fn new(cache_dir: &Path, engine: &str, key: &str) -> Self {
        Self {
            dir: cache_dir
                .join("compat")
                .join(engine)
                .join(fnv(key.as_bytes())),
        }
    }

    /// The listing of snapshot `id`, loading (and caching) it with `load` the first time.
    /// `stamp` changes when the snapshot does (so a snapshot replaced by hand is relisted).
    pub fn get(
        &self,
        id: &str,
        stamp: &str,
        load: impl FnOnce() -> io::Result<Vec<Entry>>,
    ) -> io::Result<Vec<Entry>> {
        let path = self.dir.join(format!(
            "{}.{}.zst",
            fnv(id.as_bytes()),
            fnv(stamp.as_bytes())
        ));
        if let Ok(v) = ManifestReader::read_all(&path) {
            return Ok(v);
        }
        let entries = load()?;
        if let Err(e) = self.store(&path, &entries) {
            tracing::debug!("not caching the listing of {id}: {e}");
        }
        Ok(entries)
    }

    fn store(&self, path: &Path, entries: &[Entry]) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "tmp{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut w = ManifestWriter::create(&tmp).map_err(io::Error::other)?;
        for e in entries {
            w.write(e).map_err(io::Error::other)?;
        }
        w.finish().map_err(io::Error::other)?;
        std::fs::rename(&tmp, path)
    }

    /// Forget a deleted snapshot's listing.
    pub fn forget(&self, id: &str) {
        let prefix = format!("{}.", fnv(id.as_bytes()));
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

pub(crate) fn fnv(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_find_output() {
        let raw =
            b"d\x00755\x001000\x001000\x004096\x001700000000.5\x002\x0011\x0042\x00\x00docs\x00\
f\x00644\x001000\x001000\x005\x001700000001.123456789\x002\x0012\x0042\x00\x00docs/a b\x00\
l\x00777\x001000\x001000\x001\x001700000002.0000000000\x001\x0013\x0042\x00a b\x00docs/link\x00";
        let v = parse_find(raw).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, b"docs");
        assert_eq!(v[0].kind, Kind::Dir);
        assert_eq!(v[0].mtime_nsec, 500_000_000);
        assert_eq!(v[0].size, 0);
        assert_eq!(v[1].path, b"docs/a b");
        assert_eq!(
            (v[1].size, v[1].mode, v[1].nlink, v[1].ino),
            (5, 0o644, 2, 12)
        );
        assert_eq!(v[1].mtime_nsec, 123_456_789);
        assert_eq!(v[2].link.as_deref(), Some(&b"a b"[..]));
    }

    #[test]
    fn local_scan_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("snap/data");
        std::fs::create_dir_all(data.join("home/me")).unwrap();
        std::fs::write(data.join("home/me/a.txt"), b"hello").unwrap();
        let v = scan_local(
            &data,
            XattrPolicy {
                acl: true,
                other: true,
            },
        )
        .unwrap();
        let paths: Vec<_> = v.iter().map(|e| String::from_utf8_lossy(&e.path)).collect();
        assert_eq!(paths, ["home", "home/me", "home/me/a.txt"]);
        assert_eq!(v[2].size, 5);

        let cache = ListingCache::new(&dir.path().join("cache"), "rsync", "repo");
        let first = cache.get("s1", "x", || Ok(v.clone())).unwrap();
        let again = cache.get("s1", "x", || panic!("cached")).unwrap();
        assert_eq!(first, again);
        cache.forget("s1");
        let reloaded = cache.get("s1", "x", || Ok(Vec::new())).unwrap();
        assert!(reloaded.is_empty());
    }
}
