//! Snapshot export: files out of a snapshot as a standard archive.
//!
//! - `.tar.zst`: POSIX (pax) tar compressed with zstd. Owners, modes, times (to the
//!   nanosecond), symlinks, hard links, device files and FIFOs; extended attributes as
//!   `SCHILY.xattr.*` records and POSIX ACLs as `SCHILY.acl.access`/`.default` (what GNU
//!   tar `--xattrs --acls` and bsdtar read).
//! - `.zip`: for sharing. Files and folders with their times and basic permissions;
//!   symlinks, device files and FIFOs are left out, and owners, extended attributes and
//!   ACLs aren't kept (ZIP has no place for them).
//!
//! The selection is restored into a private staging folder next to the archive (every
//! engine can restore), and the archive is written from it, with the metadata taken from
//! the snapshot itself (so owners are right even when not run as root). An optional
//! manifest lists every file's SHA-256 (`sha256sum -c`) or BLAKE3 (`b3sum -c`).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_fs::entry::{Entry, Kind, is_clean_path, is_within};
use flux_fs::restore::RestoreRequest;
use flux_fs::snapshot::SnapshotMeta;
use serde::{Deserialize, Serialize};

use crate::repo::AnyRepo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    TarZst,
    Zip,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::TarZst => "tar.zst",
            Format::Zip => "zip",
        }
    }

    /// From a file name's ending (`.tar.zst`, `.tzst`, `.zip`).
    pub fn from_name(name: &str) -> Option<Format> {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".tar.zst") || n.ends_with(".tzst") {
            Some(Format::TarZst)
        } else if n.ends_with(".zip") {
            Some(Format::Zip)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checksums {
    Sha256,
    Blake3,
}

impl Checksums {
    pub fn extension(self) -> &'static str {
        match self {
            Checksums::Sha256 => "sha256",
            Checksums::Blake3 => "b3",
        }
    }
}

pub struct ExportRequest<'a> {
    pub snapshot: &'a SnapshotMeta,
    /// Absolute paths to export; empty: the whole snapshot.
    pub paths: Vec<PathBuf>,
    pub format: Format,
    /// The archive to write (must not exist).
    pub out: PathBuf,
    pub checksums: Option<Checksums>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ExportOutcome {
    pub archive: PathBuf,
    /// Its size.
    pub size: u64,
    pub files: u64,
    /// Of the files, before compression.
    pub bytes: u64,
    pub manifest: Option<PathBuf>,
    /// What the format couldn't hold, in words ("2 symbolic links left out").
    pub left_out: Vec<String>,
    /// Files that couldn't be read from the backup.
    pub errors: Vec<String>,
}

/// What ZIP can't keep, said once with every ZIP export.
pub const ZIP_LIMITS: &str = "ZIP keeps files, folders, their times and basic permissions. It doesn't keep owners, extended attributes or ACLs, symbolic links or special files; hard links become separate copies.";

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = make_writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Folders restored read-only can't be emptied: give the owner write access first.
fn make_writable(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for e in fs::read_dir(dir)?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let p = e.path();
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
            let _ = make_writable(&p);
        }
    }
    Ok(())
}

/// Export `req.paths` (or everything) of a snapshot of `repo` as an archive.
pub fn export(
    repo: &AnyRepo,
    req: &ExportRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<ExportOutcome, String> {
    if req.out.exists() {
        return Err(format!("{} exists already", req.out.display()));
    }
    let dir = req
        .out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !dir.is_dir() {
        return Err(format!("{} isn't a folder", dir.display()));
    }
    let snap = req.snapshot;
    let wanted: Vec<Vec<u8>> = if req.paths.is_empty() {
        snap.sources
            .iter()
            .map(|p| strip(p.as_os_str().as_bytes()))
            .collect()
    } else {
        req.paths
            .iter()
            .map(|p| strip(p.as_os_str().as_bytes()))
            .collect()
    };
    progress.set_phase(Phase::Scanning);
    let mut entries: Vec<(Entry, usize)> = Vec::new();
    let mut found = vec![false; wanted.len()];
    for e in repo.entries(snap).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if !is_clean_path(&e.path) {
            continue;
        }
        if let Some(i) = wanted.iter().position(|w| is_within(&e.path, w)) {
            found[i] = true;
            entries.push((e, i));
        }
    }
    if let Some(i) = found.iter().position(|f| !f) {
        return Err(format!(
            "/{} isn't in snapshot {}",
            String::from_utf8_lossy(&wanted[i]),
            snap.id
        ));
    }
    entries.sort_by(|a, b| a.0.path.cmp(&b.0.path));

    // The files' contents, restored next to the archive (removed whatever happens).
    let staging = Staging(dir.join(format!(".flux-export-{}-{}", std::process::id(), snap.id)));
    // A folder of our own, made new and private (as root this is someone else's folder
    // as often as not), and held open: the files are read back from *this* folder,
    // whatever is done to its name meanwhile, and no link in it is followed.
    let _ = fs::remove_dir_all(&staging.0);
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&staging.0)
            .map_err(|e| format!("{}: {e}", staging.0.display()))?;
    }
    let staging_dir = {
        use rustix::fs::{Mode, OFlags};
        rustix::fs::open(
            &staging.0,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("{}: {e}", staging.0.display()))?
    };
    let mut errors: Vec<String> = Vec::new();
    for (i, w) in wanted.iter().enumerate() {
        let r = RestoreRequest {
            snapshot: snap,
            paths: vec![PathBuf::from("/").join(std::ffi::OsStr::from_bytes(w))],
            to: Some(staging.0.join(i.to_string())),
            keep_existing: true,
            dry_run: false,
            xattrs: flux_fs::fsops::XattrPolicy {
                acl: false,
                other: false,
            },
            links: flux_fs::safe::LinkPolicy::for_process(),
            exact: None,
        };
        let out = repo
            .restore(&r, progress, report)
            .map_err(|e| e.to_string())?;
        errors.extend(
            out.errors
                .into_iter()
                .map(|e| format!("{}: {}", e.path, e.message)),
        );
    }
    if progress.is_cancelled() {
        return Err("cancelled".into());
    }
    let staged = |e: &Entry, i: usize| -> PathBuf {
        let w = &wanted[i];
        let base = w.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1);
        // Relative to the staging folder.
        PathBuf::from(i.to_string()).join(std::ffi::OsStr::from_bytes(
            &e.path[base.min(e.path.len())..],
        ))
    };
    // As root, what is exported is root's to read (a system backup holds /etc/shadow):
    // the archive and its checksums are for root only, like the files of a restore. Your
    // own exports get the usual permissions.
    let private = flux_core::sys::is_root();

    progress.set_phase(Phase::Transferring);
    let total: u64 = entries
        .iter()
        .filter(|(e, _)| e.kind == Kind::File)
        .map(|(e, _)| e.size)
        .sum();
    progress.bytes_total.store(total, Relaxed);
    progress.entries_total.store(entries.len() as u64, Relaxed);
    let partial = req.out.with_file_name(format!(
        ".{}.partial",
        req.out
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let mut outcome = ExportOutcome {
        archive: req.out.clone(),
        ..Default::default()
    };
    let mut manifest: Vec<(String, Vec<u8>)> = Vec::new();
    let result = (|| -> io::Result<()> {
        // Made new, never an existing file or a link someone put there.
        let _ = fs::remove_file(&partial);
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if private { 0o600 } else { 0o666 })
                .open(&partial)?
        };
        let mut w = Writer::new(req.format, file)?;
        let mut links: HashMap<(u64, u64), Vec<u8>> = HashMap::new();
        let (mut symlinks, mut specials, mut copies) = (0u64, 0u64, 0u64);
        for (e, i) in &entries {
            if progress.is_cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            progress.entries_done.fetch_add(1, Relaxed);
            match e.kind {
                Kind::Dir => w.dir(e)?,
                Kind::File => {
                    // A hard link to a file already in the archive.
                    if e.nlink > 1 {
                        let key = (e.dev, e.ino);
                        if let Some(first) = links.get(&key) {
                            if w.hard_link(e, first)? {
                                continue;
                            }
                            copies += 1;
                        } else {
                            links.insert(key, e.path.clone());
                        }
                    }
                    let path = staged(e, *i);
                    let opened = staging_dir
                        .try_clone()
                        .and_then(|d| flux_fs::fsops::open_below_dir(d, &path));
                    let mut f = match opened {
                        Ok(f) => f,
                        Err(err) => {
                            errors.push(format!("/{}: {err}", String::from_utf8_lossy(&e.path)));
                            continue;
                        }
                    };
                    let size = f.metadata()?.len();
                    let mut hasher = req.checksums.map(Hasher::new);
                    w.file(e, size, &mut f, &mut |chunk: &[u8]| {
                        if let Some(h) = hasher.as_mut() {
                            h.update(chunk);
                        }
                        progress.bytes_done.fetch_add(chunk.len() as u64, Relaxed);
                    })?;
                    if let Some(h) = hasher {
                        manifest.push((name_of(e), h.finish()));
                    }
                    outcome.files += 1;
                    outcome.bytes += size;
                }
                Kind::Symlink => {
                    if !w.symlink(e)? {
                        symlinks += 1;
                    }
                }
                Kind::Socket => specials += 1,
                Kind::Fifo | Kind::CharDevice | Kind::BlockDevice => {
                    if !w.special(e)? {
                        specials += 1;
                    }
                }
            }
        }
        let file = w.finish()?;
        file.sync_all()?;
        let n = |c: u64, one: &str, many: &str| format!("{c} {}", if c == 1 { one } else { many });
        if symlinks > 0 {
            outcome.left_out.push(format!(
                "{} left out",
                n(symlinks, "symbolic link", "symbolic links")
            ));
        }
        if specials > 0 {
            outcome.left_out.push(format!(
                "{} left out",
                n(
                    specials,
                    "special file (socket, pipe or device)",
                    "special files (sockets, pipes or devices)"
                )
            ));
        }
        if copies > 0 {
            outcome.left_out.push(format!(
                "{} stored as separate copies",
                n(copies, "hard link", "hard links")
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&partial);
        return Err(if e.to_string() == "cancelled" {
            "cancelled".into()
        } else {
            format!("{}: {e}", req.out.display())
        });
    }
    fs::rename(&partial, &req.out).map_err(|e| format!("{}: {e}", req.out.display()))?;
    outcome.size = fs::metadata(&req.out).map(|m| m.len()).unwrap_or(0);
    if let Some(kind) = req.checksums {
        let path = PathBuf::from(format!("{}.{}", req.out.display(), kind.extension()));
        let mut text = String::new();
        for (name, digest) in &manifest {
            text.push_str(&manifest_line(name, digest));
        }
        flux_core::store::write_atomic(&path, text.as_bytes(), if private { 0o600 } else { 0o644 })
            .map_err(|e| e.to_string())?;
        outcome.manifest = Some(path);
    }
    for e in &errors {
        report(Event::Warning {
            path: String::new(),
            message: e.clone(),
        });
    }
    outcome.errors = errors;
    progress.set_phase(Phase::Done);
    Ok(outcome)
}

fn strip(b: &[u8]) -> Vec<u8> {
    let b = b.strip_prefix(b"/").unwrap_or(b);
    b.strip_suffix(b"/").unwrap_or(b).to_vec()
}

fn name_of(e: &Entry) -> String {
    String::from_utf8_lossy(&e.path).into_owned()
}

/// A line of a `sha256sum`/`b3sum` manifest, escaped as coreutils does for names with a
/// backslash or a newline.
fn manifest_line(name: &str, digest: &[u8]) -> String {
    if name.contains('\\') || name.contains('\n') {
        let esc = name.replace('\\', "\\\\").replace('\n', "\\n");
        format!("\\{}  {esc}\n", hex::encode(digest))
    } else {
        format!("{}  {name}\n", hex::encode(digest))
    }
}

enum Hasher {
    Sha256(sha2::Sha256),
    Blake3(Box<blake3::Hasher>),
}

impl Hasher {
    fn new(kind: Checksums) -> Self {
        use sha2::Digest;
        match kind {
            Checksums::Sha256 => Hasher::Sha256(sha2::Sha256::new()),
            Checksums::Blake3 => Hasher::Blake3(Box::new(blake3::Hasher::new())),
        }
    }

    fn update(&mut self, b: &[u8]) {
        use sha2::Digest;
        match self {
            Hasher::Sha256(h) => h.update(b),
            Hasher::Blake3(h) => {
                h.update(b);
            }
        }
    }

    fn finish(self) -> Vec<u8> {
        use sha2::Digest;
        match self {
            Hasher::Sha256(h) => h.finalize().to_vec(),
            Hasher::Blake3(h) => h.finalize().as_bytes().to_vec(),
        }
    }
}

// ---- writers ----

enum Writer {
    Tar(Tar<zstd::stream::write::Encoder<'static, BufWriter<File>>>),
    Zip(Zip),
}

impl Writer {
    fn new(format: Format, file: File) -> io::Result<Self> {
        Ok(match format {
            Format::TarZst => {
                let mut enc = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
                enc.include_checksum(true)?;
                Writer::Tar(Tar { out: enc })
            }
            Format::Zip => Writer::Zip(Zip::new(file)),
        })
    }

    fn dir(&mut self, e: &Entry) -> io::Result<()> {
        match self {
            Writer::Tar(t) => t.entry(e, b'5', 0, None, None),
            Writer::Zip(z) => z.dir(e),
        }
    }

    fn file(
        &mut self,
        e: &Entry,
        size: u64,
        data: &mut File,
        seen: &mut dyn FnMut(&[u8]),
    ) -> io::Result<()> {
        match self {
            Writer::Tar(t) => t.entry(e, b'0', size, None, Some((data, seen))),
            Writer::Zip(z) => z.file(e, size, data, seen),
        }
    }

    /// `false`: the format can't hold one (the file is stored again instead).
    fn hard_link(&mut self, e: &Entry, first: &[u8]) -> io::Result<bool> {
        match self {
            Writer::Tar(t) => t.entry(e, b'1', 0, Some(first), None).map(|_| true),
            Writer::Zip(_) => Ok(false),
        }
    }

    fn symlink(&mut self, e: &Entry) -> io::Result<bool> {
        match self {
            Writer::Tar(t) => {
                let target = e.link.clone().unwrap_or_default();
                t.entry(e, b'2', 0, Some(&target), None).map(|_| true)
            }
            Writer::Zip(_) => Ok(false),
        }
    }

    fn special(&mut self, e: &Entry) -> io::Result<bool> {
        match self {
            Writer::Tar(t) => {
                let flag = match e.kind {
                    Kind::CharDevice => b'3',
                    Kind::BlockDevice => b'4',
                    _ => b'6',
                };
                t.entry(e, flag, 0, None, None).map(|_| true)
            }
            Writer::Zip(_) => Ok(false),
        }
    }

    fn finish(self) -> io::Result<File> {
        match self {
            Writer::Tar(mut t) => {
                t.out.write_all(&[0u8; 1024])?;
                let buf = t.out.finish()?;
                buf.into_inner().map_err(|e| e.into_error())
            }
            Writer::Zip(z) => z.finish(),
        }
    }
}

/// A file's contents for the archive, and what sees each piece as it's written (the
/// checksum, the progress).
type FileData<'a> = (&'a mut File, &'a mut dyn FnMut(&[u8]));

/// A POSIX (pax) tar stream.
struct Tar<W: Write> {
    out: W,
}

fn octal(field: &mut [u8], v: u64) -> bool {
    let digits = field.len() - 1;
    let s = format!("{v:o}");
    if s.len() > digits {
        return false;
    }
    let pad = digits - s.len();
    field[..pad].fill(b'0');
    field[pad..digits].copy_from_slice(s.as_bytes());
    field[digits] = 0;
    true
}

fn pax_record(key: &str, value: &[u8]) -> Vec<u8> {
    // "<len> key=value\n", where <len> counts itself.
    let body = key.len() + value.len() + 3; // ' ', '=', '\n'
    let mut len = body + 1;
    loop {
        let total = body + len.to_string().len();
        if total == len {
            break;
        }
        len = total;
    }
    let mut r = format!("{len} {key}=").into_bytes();
    r.extend_from_slice(value);
    r.push(b'\n');
    r
}

/// POSIX ACL xattr (`system.posix_acl_*`) as text with numeric ids ("user::rw-,user:1000:r--,…").
fn acl_text(raw: &[u8]) -> Option<String> {
    if raw.len() < 4 || u32::from_le_bytes(raw[..4].try_into().ok()?) != 2 {
        return None;
    }
    let mut parts = Vec::new();
    for c in raw[4..].chunks(8) {
        if c.len() != 8 {
            return None;
        }
        let tag = u16::from_le_bytes([c[0], c[1]]);
        let perm = u16::from_le_bytes([c[2], c[3]]);
        let id = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
        let p = format!(
            "{}{}{}",
            if perm & 4 != 0 { 'r' } else { '-' },
            if perm & 2 != 0 { 'w' } else { '-' },
            if perm & 1 != 0 { 'x' } else { '-' }
        );
        parts.push(match tag {
            0x01 => format!("user::{p}"),
            0x02 => format!("user:{id}:{p}"),
            0x04 => format!("group::{p}"),
            0x08 => format!("group:{id}:{p}"),
            0x10 => format!("mask::{p}"),
            0x20 => format!("other::{p}"),
            _ => return None,
        });
    }
    Some(parts.join(","))
}

impl<W: Write> Tar<W> {
    fn entry(
        &mut self,
        e: &Entry,
        flag: u8,
        size: u64,
        link: Option<&[u8]>,
        data: Option<FileData<'_>>,
    ) -> io::Result<()> {
        let mut name = e.path.clone();
        if flag == b'5' {
            name.push(b'/');
        }
        // pax records: the exact path, link, times, large numbers, xattrs and ACLs.
        let mut pax = Vec::new();
        pax.extend(pax_record("path", &name));
        if let Some(l) = link {
            pax.extend(pax_record("linkpath", l));
        }
        let mtime = if e.mtime >= 0 {
            format!("{}.{:09}", e.mtime, e.mtime_nsec)
        } else {
            format!("{}", e.mtime)
        };
        pax.extend(pax_record("mtime", mtime.as_bytes()));
        let mut h = [0u8; 512];
        // The ustar fields, as far as they go (readers take the pax records first).
        let short: Vec<u8> = name
            .iter()
            .rev()
            .take(99)
            .rev()
            .map(|&b| if b.is_ascii() { b } else { b'_' })
            .collect();
        h[..short.len()].copy_from_slice(&short);
        octal(&mut h[100..108], u64::from(e.mode & 0o7777));
        if !octal(&mut h[108..116], u64::from(e.uid)) {
            pax.extend(pax_record("uid", e.uid.to_string().as_bytes()));
            octal(&mut h[108..116], 0);
        }
        if !octal(&mut h[116..124], u64::from(e.gid)) {
            pax.extend(pax_record("gid", e.gid.to_string().as_bytes()));
            octal(&mut h[116..124], 0);
        }
        if !octal(&mut h[124..136], size) {
            pax.extend(pax_record("size", size.to_string().as_bytes()));
            octal(&mut h[124..136], 0);
        }
        octal(&mut h[136..148], e.mtime.max(0) as u64);
        h[156] = flag;
        if let Some(l) = link {
            let n = l.len().min(100);
            h[157..157 + n].copy_from_slice(&l[..n]);
        }
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        if matches!(flag, b'3' | b'4') {
            let major = ((e.rdev >> 8) & 0xfff) | ((e.rdev >> 32) & !0xfff);
            let minor = (e.rdev & 0xff) | ((e.rdev >> 12) & !0xff);
            octal(&mut h[329..337], major);
            octal(&mut h[337..345], minor);
        }
        for x in &e.xattrs {
            if x.name == b"system.posix_acl_access" || x.name == b"system.posix_acl_default" {
                if let Some(t) = acl_text(&x.value) {
                    let key = if x.name.ends_with(b"access") {
                        "SCHILY.acl.access"
                    } else {
                        "SCHILY.acl.default"
                    };
                    pax.extend(pax_record(key, t.as_bytes()));
                }
            } else if let Ok(n) = std::str::from_utf8(&x.name) {
                pax.extend(pax_record(&format!("SCHILY.xattr.{n}"), &x.value));
            }
        }
        // The extended header, then the entry.
        let mut x = [0u8; 512];
        x[..12].copy_from_slice(b"PaxHeader/x\0");
        octal(&mut x[100..108], 0o644);
        octal(&mut x[108..116], 0);
        octal(&mut x[116..124], 0);
        octal(&mut x[124..136], pax.len() as u64);
        octal(&mut x[136..148], e.mtime.max(0) as u64);
        x[156] = b'x';
        x[257..263].copy_from_slice(b"ustar\0");
        x[263..265].copy_from_slice(b"00");
        checksum(&mut x);
        self.out.write_all(&x)?;
        self.out.write_all(&pax)?;
        self.pad(pax.len() as u64)?;
        checksum(&mut h);
        self.out.write_all(&h)?;
        if let Some((f, seen)) = data {
            let mut left = size;
            let mut buf = vec![0u8; 1 << 20];
            while left > 0 {
                let n = f.read(&mut buf[..left.min(1 << 20) as usize])?;
                if n == 0 {
                    return Err(io::Error::other(
                        "the file got shorter while being archived",
                    ));
                }
                seen(&buf[..n]);
                self.out.write_all(&buf[..n])?;
                left -= n as u64;
            }
            self.pad(size)?;
        }
        Ok(())
    }

    fn pad(&mut self, len: u64) -> io::Result<()> {
        let rem = (len % 512) as usize;
        if rem != 0 {
            self.out.write_all(&[0u8; 512][..512 - rem])?;
        }
        Ok(())
    }
}

fn checksum(h: &mut [u8; 512]) {
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
}

/// A ZIP archive (ZIP64 where needed), written to a seekable file.
struct Zip {
    out: File,
    pos: u64,
    central: Vec<u8>,
    count: u64,
}

struct ZipEntry {
    name: Vec<u8>,
    dos_time: u16,
    dos_date: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
    method: u16,
    unix_mode: u32,
    mtime: i64,
}

fn dos_datetime(mtime: i64) -> (u16, u16) {
    let Ok(t) = jiff::Timestamp::from_second(mtime) else {
        return (0, 0x21);
    };
    let z = t.to_zoned(jiff::tz::TimeZone::system());
    if z.year() < 1980 {
        return (0, 0x21); // 1980-01-01
    }
    let time = ((z.hour() as u16) << 11) | ((z.minute() as u16) << 5) | (z.second() as u16 / 2);
    let date = (((z.year() - 1980) as u16) << 9) | ((z.month() as u16) << 5) | z.day() as u16;
    (time, date)
}

impl Zip {
    fn new(out: File) -> Self {
        Zip {
            out,
            pos: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    fn write(&mut self, b: &[u8]) -> io::Result<()> {
        self.out.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }

    fn name(e: &Entry, dir: bool) -> Vec<u8> {
        let mut n = String::from_utf8_lossy(&e.path).into_owned().into_bytes();
        if dir {
            n.push(b'/');
        }
        n
    }

    fn local_header(&mut self, z: &ZipEntry, zip64: bool) -> io::Result<()> {
        let mut h = Vec::with_capacity(30 + z.name.len() + 29);
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend_from_slice(&z.method.to_le_bytes());
        h.extend_from_slice(&z.dos_time.to_le_bytes());
        h.extend_from_slice(&z.dos_date.to_le_bytes());
        h.extend_from_slice(&z.crc.to_le_bytes());
        let (c, s) = if zip64 {
            (u32::MAX, u32::MAX)
        } else {
            (z.compressed as u32, z.size as u32)
        };
        h.extend_from_slice(&c.to_le_bytes());
        h.extend_from_slice(&s.to_le_bytes());
        h.extend_from_slice(&(z.name.len() as u16).to_le_bytes());
        let extra = Self::extras(z, zip64, false);
        h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        h.extend_from_slice(&z.name);
        h.extend_from_slice(&extra);
        self.write(&h)
    }

    /// The extra fields: ZIP64 sizes (and offset, in the central directory) and the
    /// modification time to the second.
    fn extras(z: &ZipEntry, zip64: bool, central: bool) -> Vec<u8> {
        let mut x = Vec::new();
        if zip64 {
            let mut body = Vec::new();
            body.extend_from_slice(&z.size.to_le_bytes());
            body.extend_from_slice(&z.compressed.to_le_bytes());
            if central {
                body.extend_from_slice(&z.offset.to_le_bytes());
            }
            x.extend_from_slice(&0x0001u16.to_le_bytes());
            x.extend_from_slice(&(body.len() as u16).to_le_bytes());
            x.extend_from_slice(&body);
        }
        if (0..=i64::from(u32::MAX)).contains(&z.mtime) {
            x.extend_from_slice(&0x5455u16.to_le_bytes());
            x.extend_from_slice(&5u16.to_le_bytes());
            x.push(1);
            x.extend_from_slice(&(z.mtime as u32).to_le_bytes());
        }
        x
    }

    fn central_entry(&mut self, z: &ZipEntry) {
        let zip64 = z.size >= u64::from(u32::MAX)
            || z.compressed >= u64::from(u32::MAX)
            || z.offset >= u64::from(u32::MAX);
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&((3u16 << 8) | 63).to_le_bytes()); // made on Unix
        c.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&z.method.to_le_bytes());
        c.extend_from_slice(&z.dos_time.to_le_bytes());
        c.extend_from_slice(&z.dos_date.to_le_bytes());
        c.extend_from_slice(&z.crc.to_le_bytes());
        let big = |v: u64| if zip64 { u32::MAX } else { v as u32 };
        c.extend_from_slice(&big(z.compressed).to_le_bytes());
        c.extend_from_slice(&big(z.size).to_le_bytes());
        c.extend_from_slice(&(z.name.len() as u16).to_le_bytes());
        let extra = Self::extras(z, zip64, true);
        c.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        c.extend_from_slice(&(z.unix_mode << 16).to_le_bytes());
        c.extend_from_slice(&big(z.offset).to_le_bytes());
        c.extend_from_slice(&z.name);
        c.extend_from_slice(&extra);
        self.count += 1;
    }

    fn dir(&mut self, e: &Entry) -> io::Result<()> {
        let (dos_time, dos_date) = dos_datetime(e.mtime);
        let z = ZipEntry {
            name: Self::name(e, true),
            dos_time,
            dos_date,
            crc: 0,
            compressed: 0,
            size: 0,
            offset: self.pos,
            method: 0,
            unix_mode: 0o040000 | (e.mode & 0o777),
            mtime: e.mtime,
        };
        self.local_header(&z, false)?;
        self.central_entry(&z);
        Ok(())
    }

    fn file(
        &mut self,
        e: &Entry,
        size: u64,
        data: &mut File,
        seen: &mut dyn FnMut(&[u8]),
    ) -> io::Result<()> {
        let (dos_time, dos_date) = dos_datetime(e.mtime);
        // Big files get ZIP64 sizes from the start (deflate can grow data a little).
        let zip64 = size >= 0xFFFF_0000;
        let mut z = ZipEntry {
            name: Self::name(e, false),
            dos_time,
            dos_date,
            crc: 0,
            compressed: 0,
            size,
            offset: self.pos,
            method: 8,
            unix_mode: 0o100000 | (e.mode & 0o777),
            mtime: e.mtime,
        };
        self.local_header(&z, zip64)?;
        let start = self.pos;
        let mut crc = crc32fast::Hasher::new();
        {
            let mut enc = flate2::write::DeflateEncoder::new(
                Counting {
                    out: &mut self.out,
                    n: 0,
                },
                flate2::Compression::default(),
            );
            let mut buf = vec![0u8; 1 << 20];
            let mut left = size;
            while left > 0 {
                let n = data.read(&mut buf[..left.min(1 << 20) as usize])?;
                if n == 0 {
                    return Err(io::Error::other(
                        "the file got shorter while being archived",
                    ));
                }
                crc.update(&buf[..n]);
                seen(&buf[..n]);
                enc.write_all(&buf[..n])?;
                left -= n as u64;
            }
            let c = enc.finish()?;
            self.pos += c.n;
        }
        z.crc = crc.finalize();
        z.compressed = self.pos - start;
        if !zip64 && z.compressed >= u64::from(u32::MAX) {
            return Err(io::Error::other("a file grew past 4 GB when compressed"));
        }
        // The CRC and sizes, now known, into the local header.
        let end = self.pos;
        self.out.seek(SeekFrom::Start(z.offset + 14))?;
        self.out.write_all(&z.crc.to_le_bytes())?;
        if zip64 {
            let extra_at = z.offset + 30 + z.name.len() as u64 + 4;
            self.out.seek(SeekFrom::Start(extra_at))?;
            self.out.write_all(&z.size.to_le_bytes())?;
            self.out.write_all(&z.compressed.to_le_bytes())?;
        } else {
            self.out.write_all(&(z.compressed as u32).to_le_bytes())?;
            self.out.write_all(&(z.size as u32).to_le_bytes())?;
        }
        self.out.seek(SeekFrom::Start(end))?;
        self.central_entry(&z);
        Ok(())
    }

    fn finish(mut self) -> io::Result<File> {
        let cd_offset = self.pos;
        let central = std::mem::take(&mut self.central);
        self.write(&central)?;
        let cd_size = central.len() as u64;
        let zip64 = self.count >= 0xFFFF
            || cd_offset >= u64::from(u32::MAX)
            || cd_size >= u64::from(u32::MAX);
        let mut t = Vec::new();
        if zip64 {
            let eocd64 = self.pos;
            t.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
            t.extend_from_slice(&44u64.to_le_bytes());
            t.extend_from_slice(&((3u16 << 8) | 63).to_le_bytes());
            t.extend_from_slice(&45u16.to_le_bytes());
            t.extend_from_slice(&0u32.to_le_bytes());
            t.extend_from_slice(&0u32.to_le_bytes());
            t.extend_from_slice(&self.count.to_le_bytes());
            t.extend_from_slice(&self.count.to_le_bytes());
            t.extend_from_slice(&cd_size.to_le_bytes());
            t.extend_from_slice(&cd_offset.to_le_bytes());
            t.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
            t.extend_from_slice(&0u32.to_le_bytes());
            t.extend_from_slice(&eocd64.to_le_bytes());
            t.extend_from_slice(&1u32.to_le_bytes());
        }
        let small = |v: u64, max: u64| if zip64 { max } else { v };
        t.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes());
        t.extend_from_slice(&(small(self.count, 0xFFFF) as u16).to_le_bytes());
        t.extend_from_slice(&(small(self.count, 0xFFFF) as u16).to_le_bytes());
        t.extend_from_slice(&(small(cd_size, u64::from(u32::MAX)) as u32).to_le_bytes());
        t.extend_from_slice(&(small(cd_offset, u64::from(u32::MAX)) as u32).to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes());
        self.write(&t)?;
        Ok(self.out)
    }
}

struct Counting<'a> {
    out: &'a mut File,
    n: u64,
}

impl Write for Counting<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.out.write(b)?;
        self.n += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pax_records_count_their_own_length() {
        for (k, v) in [
            ("path", "a"),
            ("path", "x".repeat(95).as_str()),
            ("mtime", "1.5"),
        ] {
            let r = pax_record(k, v.as_bytes());
            let s = String::from_utf8(r.clone()).unwrap();
            let (len, _) = s.split_once(' ').unwrap();
            assert_eq!(len.parse::<usize>().unwrap(), r.len(), "{s:?}");
        }
    }

    #[test]
    fn acls_as_text() {
        let mut raw = 2u32.to_le_bytes().to_vec();
        for (tag, perm, id) in [
            (1u16, 6u16, u32::MAX),
            (2, 4, 1000),
            (4, 4, u32::MAX),
            (0x10, 4, u32::MAX),
            (0x20, 0, u32::MAX),
        ] {
            raw.extend_from_slice(&tag.to_le_bytes());
            raw.extend_from_slice(&perm.to_le_bytes());
            raw.extend_from_slice(&id.to_le_bytes());
        }
        assert_eq!(
            acl_text(&raw).unwrap(),
            "user::rw-,user:1000:r--,group::r--,mask::r--,other::---"
        );
        assert!(acl_text(b"junk").is_none());
    }

    #[test]
    fn manifest_lines_escape_like_coreutils() {
        assert_eq!(manifest_line("a b", &[0xab]), "ab  a b\n");
        assert_eq!(manifest_line("a\nb\\c", &[0xab]), "\\ab  a\\nb\\\\c\n");
    }

    #[test]
    fn formats_from_names() {
        assert_eq!(Format::from_name("x.tar.zst"), Some(Format::TarZst));
        assert_eq!(Format::from_name("X.ZIP"), Some(Format::Zip));
        assert_eq!(Format::from_name("x.tar"), None);
    }
}
