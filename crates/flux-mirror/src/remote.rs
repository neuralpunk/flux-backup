//! Mirror repositories on SSH targets.
//!
//! Same layout as local repositories, under `<remote path>/flux-backup/<host>/<user>/<profile>`.
//! Metadata (marker, meta.json, manifests, locks) moves over SFTP. File data moves with
//! `rsync --link-dest` (the remote needs rsync), which hard-links unchanged files on the
//! server exactly like the local engine. The file list comes from Flux's own scan and plan,
//! so filters, change detection and manifests behave the same as for local targets.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use jiff::Timestamp;

use flux_core::profile::Profile;
use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_core::{ProfileId, sys};
use flux_fs::entry::{Entry, Kind};
use flux_fs::fsops::Copier;
use flux_fs::outcome::{BackupOutcome, CheckSummary};
use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus, find_snapshot};
use flux_transport::lock::RemoteLock;
use flux_transport::{SftpPool, SshTarget, shell_quote};

use crate::MirrorError;
use crate::manifest::{ManifestReader, ManifestWriter};
use crate::plan::{Action, plan};
use crate::repo::{PARTIAL, REPO_FORMAT, REPO_MARKER, RepoInfo};

const MAX_STORED_ERRORS: usize = 1000;

fn remote_err(e: impl std::fmt::Display) -> MirrorError {
    MirrorError::Remote(e.to_string())
}

/// A Mirror repository on an SSH target.
pub struct RemoteMirror {
    pool: Arc<SftpPool>,
    root: String,
    cache: PathBuf,
}

impl RemoteMirror {
    /// Remote repository root for a profile.
    pub fn locate(remote_path: &Path, host: &str, user: &str, profile: &str) -> String {
        let base = remote_path.display().to_string();
        let base = base.trim_end_matches('/');
        let base = if base.is_empty() { "." } else { base };
        format!("{base}/flux-backup/{host}/{user}/{profile}")
    }

    fn new(pool: Arc<SftpPool>, root: String, cache_dir: &Path) -> Self {
        let key = fnv(format!("{}:{root}", pool.target().destination()).as_bytes());
        Self {
            pool,
            root,
            cache: cache_dir.join("remote-mirror").join(key),
        }
    }

    pub fn open(pool: Arc<SftpPool>, root: String, cache_dir: &Path) -> Result<Self, MirrorError> {
        let r = Self::new(pool, root, cache_dir);
        let marker = format!("{}/{REPO_MARKER}", r.root);
        match r.pool.with(|s| s.read_file(&marker)) {
            Ok(raw) => {
                let info: RepoInfo = toml::from_str(&String::from_utf8_lossy(&raw))
                    .map_err(|e| MirrorError::Corrupt(format!("{marker}: {e}")))?;
                if info.engine != "mirror" || info.format > REPO_FORMAT {
                    return Err(MirrorError::Corrupt(format!(
                        "{marker}: not a supported mirror repository"
                    )));
                }
                Ok(r)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(MirrorError::NoRepository(PathBuf::from(r.describe())))
            }
            Err(e) => Err(remote_err(e)),
        }
    }

    pub fn open_or_init(
        pool: Arc<SftpPool>,
        root: String,
        cache_dir: &Path,
        profile: &str,
    ) -> Result<Self, MirrorError> {
        match Self::open(pool.clone(), root.clone(), cache_dir) {
            Err(MirrorError::NoRepository(_)) => {
                let r = Self::new(pool, root, cache_dir);
                let info = RepoInfo {
                    format: REPO_FORMAT,
                    engine: "mirror".into(),
                    profile: profile.into(),
                    host: sys::hostname(),
                    user: sys::username(),
                    created: Timestamp::now(),
                };
                let text = toml::to_string_pretty(&info)
                    .map_err(|e| MirrorError::Corrupt(e.to_string()))?;
                r.pool
                    .with(|s| s.mkdir_p(&format!("{}/snapshots", r.root), 0o700))
                    .map_err(remote_err)?;
                r.put(REPO_MARKER, text.as_bytes())?;
                Ok(r)
            }
            other => other,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "ssh://{}/{}",
            self.pool.target().destination(),
            self.root.trim_start_matches('/')
        )
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn ssh(&self) -> &SshTarget {
        self.pool.target()
    }

    /// Atomically write `rel` (relative to the repository root).
    fn put(&self, rel: &str, data: &[u8]) -> Result<(), MirrorError> {
        let path = format!("{}/{rel}", self.root);
        let tmp = format!("{path}.flux-tmp");
        self.pool
            .with(|s| {
                s.write_new(&tmp, data, 0o600)?;
                s.rename(&tmp, &path)
            })
            .map_err(remote_err)
    }

    /// Exclusive repository lock (lock files; refreshed while held).
    pub fn lock(&self) -> Result<RemoteLock, MirrorError> {
        flux_transport::lock::acquire(self.pool.clone(), &format!("{}/locks", self.root), true)
            .map_err(remote_err)?
            .ok_or_else(|| MirrorError::Locked(PathBuf::from(self.describe())))
    }

    /// Read up to `len` bytes of entry `e` of snapshot `id` at `offset` (for mounting).
    pub fn read_file_range(
        &self,
        id: &str,
        e: &Entry,
        offset: u64,
        len: u64,
    ) -> std::io::Result<Vec<u8>> {
        let remote = format!(
            "{}/data/{}",
            self.snapshot_dir(id),
            String::from_utf8_lossy(&e.path)
        );
        let mut out = Vec::with_capacity(len.min(1 << 20) as usize);
        self.pool.with(|s| {
            s.read_range(&remote, offset, len, |chunk| {
                out.extend_from_slice(chunk);
                Ok(())
            })
        })?;
        Ok(out)
    }

    pub fn snapshot_dir(&self, id: &str) -> String {
        format!("{}/snapshots/{id}", self.root)
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, MirrorError> {
        let dir = format!("{}/snapshots", self.root);
        let names = match self.pool.with(|s| s.readdir(&dir)) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(remote_err(e)),
        };
        let mut out = Vec::new();
        for (name, attrs) in names {
            if name.starts_with('.') || !attrs.is_dir() {
                continue;
            }
            match self
                .pool
                .with(|s| s.read_file(&format!("{dir}/{name}/meta.json")))
            {
                Ok(raw) => match serde_json::from_slice::<SnapshotMeta>(&raw) {
                    Ok(m) => out.push(m),
                    Err(e) => tracing::warn!("skipping snapshot {name}: {e}"),
                },
                Err(e) => tracing::warn!("skipping snapshot {name}: {e}"),
            }
        }
        out.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, MirrorError> {
        find_snapshot(self.snapshots()?, spec).map_err(|e| match e {
            flux_fs::snapshot::FindError::NotFound(s) => MirrorError::SnapshotNotFound(s),
            flux_fs::snapshot::FindError::Ambiguous(s, n) => MirrorError::Ambiguous(s, n),
        })
    }

    /// Local copy of a snapshot's manifest (downloaded once; snapshots never change).
    pub fn manifest(&self, id: &str) -> Result<PathBuf, MirrorError> {
        let local = self.cache.join(format!("{id}.manifest.zst"));
        let remote = format!("{}/manifest.zst", self.snapshot_dir(id));
        let size = self
            .pool
            .with(|s| s.lstat(&remote))
            .map_err(remote_err)?
            .size;
        if std::fs::metadata(&local).ok().map(|m| m.len()) == size {
            return Ok(local);
        }
        std::fs::create_dir_all(&self.cache).map_err(|e| MirrorError::io(&self.cache, e))?;
        let data = self
            .pool
            .with(|s| s.read_file(&remote))
            .map_err(remote_err)?;
        flux_core::store::write_atomic(&local, &data, 0o600)?;
        Ok(local)
    }

    pub fn entries(&self, id: &str) -> Result<ManifestReader, MirrorError> {
        Ok(ManifestReader::open(&self.manifest(id)?)?)
    }

    pub fn update_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut SnapshotMeta),
    ) -> Result<SnapshotMeta, MirrorError> {
        let raw = self
            .pool
            .with(|s| s.read_file(&format!("{}/meta.json", self.snapshot_dir(id))))
            .map_err(|_| MirrorError::SnapshotNotFound(id.into()))?;
        let mut m: SnapshotMeta =
            serde_json::from_slice(&raw).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
        f(&mut m);
        let json =
            serde_json::to_vec_pretty(&m).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
        self.put(&format!("snapshots/{id}/meta.json"), &json)?;
        Ok(m)
    }

    /// Delete a snapshot (renamed away first, as locally). Caller holds the lock.
    pub fn delete_snapshot(&self, id: &str) -> Result<(), MirrorError> {
        if id.is_empty() || id.starts_with('.') || id.contains('/') {
            return Err(MirrorError::SnapshotNotFound(id.into()));
        }
        let snaps = shell_quote(&format!("{}/snapshots", self.root));
        let (from, doomed) = (shell_quote(id), shell_quote(&format!(".deleting-{id}")));
        self.ssh()
            .run(&format!("cd {snaps} && mv -- {from} {doomed} && chmod -R u+w -- {doomed} && rm -rf -- {doomed}"))
            .map_err(remote_err)?;
        match self.snapshots()?.pop() {
            Some(l) => self.set_latest(&l.id),
            None => self
                .ssh()
                .run(&format!(
                    "rm -f -- {}",
                    shell_quote(&format!("{}/latest", self.root))
                ))
                .map(|_| ())
                .map_err(remote_err),
        }
    }

    pub fn set_latest(&self, id: &str) -> Result<(), MirrorError> {
        let link = shell_quote(&format!("{}/latest", self.root));
        self.ssh()
            .run(&format!(
                "ln -sfn {} {link}",
                shell_quote(&format!("snapshots/{id}"))
            ))
            .map(|_| ())
            .map_err(remote_err)
    }

    /// Free bytes on the remote filesystem, if the server reports it.
    pub fn free_space(&self) -> Option<u64> {
        self.pool
            .with(|s| s.statvfs(&self.root))
            .ok()
            .map(|v| v.free_bytes())
    }
}

fn fnv(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub struct RemoteBackup<'a> {
    pub id: &'a ProfileId,
    pub profile: &'a Profile,
    pub name: Option<String>,
    pub dry_run: bool,
    pub prune: bool,
    pub cache_dir: &'a Path,
}

/// Take a Mirror snapshot on an SSH target.
pub fn backup(
    pool: Arc<SftpPool>,
    req: &RemoteBackup<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, MirrorError> {
    let started = Timestamp::now();
    let profile = req.profile;
    let flux_core::Target::Ssh {
        path,
        bandwidth_limit,
        ..
    } = &profile.target
    else {
        return Err(MirrorError::Unsupported(
            "local targets in the remote engine",
        ));
    };
    progress.set_phase(Phase::Preparing);
    report(Event::Phase {
        phase: Phase::Preparing,
    });
    let ssh = pool.target().clone();
    ssh.run("command -v rsync >/dev/null || { echo 'rsync is not installed on the server' >&2; exit 1; }")
        .map_err(remote_err)?;
    let root = RemoteMirror::locate(path, &sys::hostname(), &sys::username(), req.id.as_str());
    let repo = if req.dry_run {
        RemoteMirror::open(pool.clone(), root, req.cache_dir).ok()
    } else {
        Some(RemoteMirror::open_or_init(
            pool.clone(),
            root,
            req.cache_dir,
            req.id.as_str(),
        )?)
    };
    let _lock = repo.as_ref().map(RemoteMirror::lock).transpose()?;

    // Scan and plan locally, exactly as for local targets.
    progress.set_phase(Phase::Scanning);
    report(Event::Phase {
        phase: Phase::Scanning,
    });
    let pool_threads =
        flux_fs::scan::thread_pool(profile).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
    let scanned = pool_threads
        .install(|| flux_fs::scan::scan_profile(profile, &[], progress, report))
        .map_err(|e| match e {
            flux_fs::scan::ProfileScanError::NoSources => MirrorError::NoSources,
            flux_fs::scan::ProfileScanError::Cancelled => MirrorError::Cancelled,
            flux_fs::scan::ProfileScanError::Filter(f) => MirrorError::Filter(f),
            other => MirrorError::Corrupt(other.to_string()),
        })?;
    progress.set_phase(Phase::Planning);
    report(Event::Phase {
        phase: Phase::Planning,
    });
    let previous = match &repo {
        Some(r) => r.snapshots()?.pop(),
        None => None,
    };
    let prev_manifest = match (&repo, &previous) {
        (Some(r), Some(p)) => Some(r.entries(&p.id)?),
        _ => None,
    };
    let the_plan = plan(scanned.entries, prev_manifest)?;
    if req.dry_run {
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::DryRun {
            stats: the_plan.stats,
            errors: scanned.errors,
        });
    }
    let repo = repo.expect("opened for real runs");
    if let Some(prev) = &previous
        && !the_plan.stats.has_changes()
        && !profile.options.snapshot_if_unchanged
        && req.name.is_none()
    {
        let _ = ssh.run(&format!(
            "chmod -R u+w -- {p} 2>/dev/null; rm -rf -- {p}",
            p = shell_quote(&format!("{}/snapshots/{PARTIAL}", repo.root))
        ));
        let pruned = if req.prune {
            prune_remote(&repo, profile, report)?
        } else {
            Vec::new()
        };
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::Unchanged {
            previous: prev.id.clone(),
            pruned,
        });
    }

    let mut pruned = Vec::new();
    if let Some(free) = repo.free_space() {
        let needed = the_plan.stats.copy_bytes + the_plan.stats.copy_bytes / 50 + (16 << 20);
        if free < needed {
            if req.prune {
                pruned = make_room(&repo, profile, needed, report)?;
            }
            let free = repo.free_space().unwrap_or(u64::MAX);
            if free < needed {
                return Err(MirrorError::NoSpace {
                    needed: flux_core::ByteSize(needed).display_approx(),
                    available: flux_core::ByteSize(free).display_approx(),
                });
            }
        }
    }

    // The file list for rsync: everything planned, except what can't be recreated remotely.
    let partial = format!("{}/snapshots/{PARTIAL}", repo.root);
    pool.with(|s| s.mkdir_p(&format!("{partial}/data"), 0o700))
        .map_err(remote_err)?;
    let list_file = tempfile_in(req.cache_dir)?;
    {
        let mut w = std::io::BufWriter::new(
            std::fs::File::create(&list_file).map_err(|e| MirrorError::io(&list_file, e))?,
        );
        for (e, a) in the_plan.entries.iter().zip(&the_plan.actions) {
            if matches!(a, Action::Special) && !matches!(e.kind, Kind::Fifo) {
                continue; // sockets and device nodes stay in the manifest only
            }
            w.write_all(&e.path)
                .and_then(|()| w.write_all(&[0]))
                .map_err(|e| MirrorError::io(&list_file, e))?;
        }
        w.flush().map_err(|e| MirrorError::io(&list_file, e))?;
    }

    progress.set_phase(Phase::Transferring);
    report(Event::Phase {
        phase: Phase::Transferring,
    });
    progress
        .entries_total
        .store(the_plan.entries.len() as u64, Relaxed);
    progress
        .bytes_total
        .store(the_plan.stats.total_bytes, Relaxed);
    let opts = &profile.options;
    let mut cmd = Command::new(std::env::var("FLUX_RSYNC").unwrap_or_else(|_| "rsync".into()));
    cmd.args([
        "-a",
        "-H",
        "--numeric-ids",
        "--from0",
        "--info=progress2",
        "--no-inc-recursive",
        "--outbuf=L",
    ])
    .arg(format!("--files-from={}", list_file.display()))
    .arg("-e")
    .arg(ssh.rsync_rsh());
    if opts.preserve_acl {
        cmd.arg("-A");
    }
    if opts.preserve_xattr {
        cmd.arg("-X");
    }
    if let Some(prev) = &previous {
        // Relative to the destination directory (.partial/data).
        cmd.arg(format!("--link-dest=../../{}/data", prev.id));
    }
    if let Some(bw) = bandwidth_limit {
        cmd.arg(format!("--bwlimit={}", (bw.bytes() / 1024).max(1)));
    }
    cmd.arg("/")
        .arg(format!("{}:{}/data/", ssh.destination(), partial));
    let rsync = run_rsync(cmd, progress, report);
    let _ = std::fs::remove_file(&list_file);
    let file_errors = rsync?;
    if progress.is_cancelled() {
        return Err(MirrorError::Cancelled);
    }

    // Finalize: manifest + meta into .partial, then rename into place.
    progress.set_phase(Phase::Finalizing);
    report(Event::Phase {
        phase: Phase::Finalizing,
    });
    let failed: std::collections::HashSet<Vec<u8>> = file_errors
        .iter()
        .map(|f| f.path.trim_start_matches('/').as_bytes().to_vec())
        .collect();
    let local_manifest = tempfile_in(req.cache_dir)?;
    let mut writer = ManifestWriter::create(&local_manifest)?;
    let mut stats = SnapshotStats {
        new: the_plan.stats.new,
        changed: the_plan.stats.changed,
        unchanged: the_plan.stats.unchanged,
        deleted: the_plan.stats.deleted,
        copied_bytes: the_plan.stats.copy_bytes,
        copied_files: the_plan.stats.copy_files,
        linked_files: the_plan.stats.link_files,
        ..Default::default()
    };
    for e in the_plan
        .entries
        .iter()
        .filter(|e| !failed.contains(&e.path))
    {
        writer.write(e)?;
        stats.entries += 1;
        match e.kind {
            Kind::Dir => stats.dirs += 1,
            Kind::File => {
                stats.files += 1;
                stats.total_bytes += e.size;
            }
            Kind::Symlink => stats.symlinks += 1,
            _ => stats.specials += 1,
        }
    }
    writer.finish()?;
    let manifest_bytes =
        std::fs::read(&local_manifest).map_err(|e| MirrorError::io(&local_manifest, e))?;
    let _ = std::fs::remove_file(&local_manifest);

    let mut errors = scanned.errors;
    errors.extend(file_errors);
    let errors_total = errors.len() as u64;
    errors.truncate(MAX_STORED_ERRORS);
    let existing: std::collections::HashSet<String> =
        repo.snapshots()?.into_iter().map(|s| s.id).collect();
    let base = started
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d_%H-%M-%S")
        .to_string();
    let mut id = base.clone();
    let mut n = 2;
    while existing.contains(&id) {
        id = format!("{base}_{n}");
        n += 1;
    }
    let meta = SnapshotMeta {
        format: REPO_FORMAT,
        id: id.clone(),
        profile: req.id.to_string(),
        host: sys::hostname(),
        user: sys::username(),
        started,
        finished: Timestamp::now(),
        status: if errors_total == 0 {
            SnapshotStatus::Complete
        } else {
            SnapshotStatus::CompleteWithErrors
        },
        name: req.name.clone(),
        pinned: false,
        sources: profile.sources.paths.clone(),
        stats,
        errors,
        errors_total,
        flux_version: flux_core::VERSION.to_owned(),
    };
    repo.put(
        &format!("snapshots/{PARTIAL}/manifest.zst"),
        &manifest_bytes,
    )?;
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| MirrorError::Corrupt(e.to_string()))?;
    repo.put(&format!("snapshots/{PARTIAL}/meta.json"), &json)?;
    // Keep a local copy of the manifest: browsing this snapshot needs no download.
    let cached = repo.cache.join(format!("{id}.manifest.zst"));
    if std::fs::create_dir_all(&repo.cache).is_ok() {
        let _ = flux_core::store::write_atomic(&cached, &manifest_bytes, 0o600);
    }
    pool.with(|s| s.rename(&partial, &repo.snapshot_dir(&id)))
        .map_err(remote_err)?;
    repo.set_latest(&id)?;
    if req.prune {
        pruned.extend(prune_remote(&repo, profile, report)?);
    }
    progress.set_phase(Phase::Done);
    report(Event::Phase { phase: Phase::Done });
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned,
    })
}

fn tempfile_in(dir: &Path) -> Result<PathBuf, MirrorError> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::fs::create_dir_all(dir).map_err(|e| MirrorError::io(dir, e))?;
    Ok(dir.join(format!(
        ".flux-{}-{}",
        std::process::id(),
        N.fetch_add(1, Relaxed)
    )))
}

/// Run rsync, feeding `--info=progress2` into `progress`. Returns per-file errors for exit
/// codes 23/24 (some files failed or vanished); other failures are errors.
fn run_rsync(
    mut cmd: Command,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<Vec<FileError>, MirrorError> {
    tracing::debug!("running {cmd:?}");
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| remote_err(format!("cannot run rsync: {e}")))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let errors_thread = std::thread::spawn(move || {
        let mut out = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            out.push(line);
        }
        out
    });
    let mut buf = Vec::new();
    let mut r = BufReader::new(stdout);
    let mut byte = [0u8; 1];
    loop {
        if progress.is_cancelled() {
            let _ = child.kill();
            break;
        }
        match r.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\r' || byte[0] == b'\n' => {
                // "  1,234,567  45%  12.34MB/s  0:01:23 (xfr#12, to-chk=3/100)"
                let line = String::from_utf8_lossy(&buf);
                if let Some(first) = line.split_whitespace().next()
                    && let Ok(n) = first.replace(',', "").parse::<u64>()
                {
                    progress
                        .bytes_done
                        .store(n.min(progress.bytes_total.load(Relaxed)), Relaxed);
                    if let Some(chk) = line.split("to-chk=").nth(1)
                        && let Some((left, total)) = chk.trim_end_matches(')').split_once('/')
                        && let (Ok(l), Ok(t)) = (left.parse::<u64>(), total.parse::<u64>())
                    {
                        progress.entries_done.store(t.saturating_sub(l), Relaxed);
                    }
                }
                buf.clear();
            }
            Ok(_) => buf.push(byte[0]),
            Err(_) => break,
        }
    }
    let status = child.wait().map_err(remote_err)?;
    let stderr_lines = errors_thread.join().unwrap_or_default();
    if progress.is_cancelled() {
        return Err(MirrorError::Cancelled);
    }
    let mut file_errors = Vec::new();
    for l in &stderr_lines {
        // rsync: [sender] send_files failed to open "/home/me/x": Permission denied (13)
        if let Some(start) = l.find('"')
            && let Some(len) = l[start + 1..].find('"')
        {
            let path = &l[start + 1..start + 1 + len];
            let message = l[start + 2 + len..]
                .trim_start_matches(':')
                .trim()
                .to_owned();
            let path = if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("/{path}")
            };
            report(Event::Warning {
                path: path.clone(),
                message: message.clone(),
            });
            progress.warnings.fetch_add(1, Relaxed);
            file_errors.push(FileError { path, message });
        }
    }
    match status.code() {
        Some(0) => Ok(file_errors),
        Some(23 | 24) => {
            if file_errors.is_empty() {
                file_errors.push(FileError {
                    path: String::new(),
                    message: stderr_lines.join("; "),
                });
            }
            Ok(file_errors)
        }
        _ => Err(MirrorError::Remote(format!(
            "rsync failed ({status}): {}",
            stderr_lines
                .iter()
                .rev()
                .take(5)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ))),
    }
}

fn prune_remote(
    repo: &RemoteMirror,
    profile: &Profile,
    report: Reporter<'_>,
) -> Result<Vec<String>, MirrorError> {
    Ok(prune_locked(repo, &profile.retention, false, report)?.1)
}

/// `flux prune` for a remote Mirror repository. Caller holds the lock.
pub fn prune_locked(
    repo: &RemoteMirror,
    policy: &flux_core::profile::Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<(Vec<flux_core::retention::Decision>, Vec<String>), MirrorError> {
    let snaps = repo.snapshots()?;
    let infos: Vec<_> = snaps.iter().map(crate::prune::info).collect();
    let decisions = flux_core::retention::evaluate(
        &infos,
        policy,
        Timestamp::now(),
        &jiff::tz::TimeZone::system(),
    );
    let mut removed = Vec::new();
    for d in decisions.iter().rev().filter(|d| !d.keep) {
        if !dry_run {
            repo.delete_snapshot(&d.id)?;
            report(Event::Info {
                message: format!("pruned snapshot {}", d.id),
            });
        }
        removed.push(d.id.clone());
    }
    Ok((decisions, removed))
}

fn make_room(
    repo: &RemoteMirror,
    profile: &Profile,
    needed: u64,
    report: Reporter<'_>,
) -> Result<Vec<String>, MirrorError> {
    let infos: Vec<_> = repo.snapshots()?.iter().map(crate::prune::info).collect();
    let mut removed = Vec::new();
    for id in
        flux_core::retention::space_candidates(&infos, &profile.retention, &Default::default())
    {
        if repo.free_space().is_none_or(|f| f >= needed) {
            break;
        }
        repo.delete_snapshot(&id)?;
        report(Event::Info {
            message: format!("removed old snapshot {id} to make room"),
        });
        removed.push(id);
    }
    Ok(removed)
}

/// A remote Mirror snapshot as a restore source: manifest from the cache, file contents
/// over SFTP.
pub struct RemoteSource<'a> {
    repo: &'a RemoteMirror,
    id: String,
}

impl<'a> RemoteSource<'a> {
    pub fn new(repo: &'a RemoteMirror, id: &str) -> Self {
        Self {
            repo,
            id: id.to_owned(),
        }
    }
}

impl flux_fs::restore::RestoreSource for RemoteSource<'_> {
    fn entries(&self) -> std::io::Result<Box<dyn Iterator<Item = std::io::Result<Entry>> + '_>> {
        let r = self.repo.entries(&self.id).map_err(std::io::Error::other)?;
        Ok(Box::new(r.map(|e| e.map_err(std::io::Error::other))))
    }

    fn write_file(
        &self,
        e: &Entry,
        out: &std::fs::File,
        _copier: &Copier,
        progress: &Progress,
    ) -> std::io::Result<u64> {
        let mut out = out;
        let remote = format!(
            "{}/data/{}",
            self.repo.snapshot_dir(&self.id),
            String::from_utf8_lossy(&e.path)
        );
        let n = self.repo.pool.with(|s| {
            s.read_range(&remote, 0, u64::MAX, |chunk| {
                if progress.is_cancelled() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cancelled",
                    ));
                }
                out.write_all(chunk)?;
                progress.bytes_done.fetch_add(chunk.len() as u64, Relaxed);
                Ok(())
            })
        })?;
        out.sync_all()?;
        Ok(n)
    }
}

/// Verify a remote Mirror repository: manifests decode, and every file listed exists on
/// the server with the right size (one `find` per snapshot). `--read-data` isn't
/// supported remotely.
pub fn check(
    repo: &RemoteMirror,
    read_data: bool,
    progress: &Progress,
) -> Result<CheckSummary, MirrorError> {
    progress.set_phase(Phase::Scanning);
    let snaps = repo.snapshots()?;
    let mut sum = CheckSummary {
        snapshots: snaps.len() as u64,
        ..Default::default()
    };
    if read_data {
        sum.warnings.push(
            "reading all data back isn't supported for SSH Mirror targets; checked sizes only"
                .into(),
        );
    }
    for s in &snaps {
        let entries: Vec<Entry> = match repo
            .entries(&s.id)
            .and_then(|r| r.collect::<Result<_, _>>().map_err(MirrorError::from))
        {
            Ok(e) => e,
            Err(e) => {
                sum.errors.push(format!("snapshot {}: {e}", s.id));
                continue;
            }
        };
        let data = format!("{}/data", repo.snapshot_dir(&s.id));
        let listing = repo
            .ssh()
            .run(&format!(
                "cd {} && find . -printf '%P\\t%y\\t%s\\n'",
                shell_quote(&data)
            ))
            .map_err(remote_err)?;
        let mut remote: std::collections::HashMap<&str, (&str, u64)> =
            std::collections::HashMap::new();
        for line in listing.lines() {
            let mut f = line.rsplitn(3, '\t');
            if let (Some(size), Some(kind), Some(path)) = (f.next(), f.next(), f.next()) {
                remote.insert(path, (kind, size.parse().unwrap_or(0)));
            }
        }
        for e in &entries {
            let p = String::from_utf8_lossy(&e.path);
            let problem = match (remote.get(p.as_ref()), e.kind) {
                (None, Kind::File | Kind::Dir | Kind::Symlink) => Some("missing".to_owned()),
                (Some(&(k, n)), Kind::File) if k != "f" || n != e.size => {
                    Some(format!("size {n} instead of {}", e.size))
                }
                _ => None,
            };
            if let Some(msg) = problem {
                sum.errors.push(format!("snapshot {}: /{p}: {msg}", s.id));
            }
            sum.checked += 1;
        }
    }
    progress.set_phase(Phase::Done);
    Ok(sum)
}
