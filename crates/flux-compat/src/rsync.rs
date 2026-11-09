//! The rsync engine: every snapshot is a plain folder written by the system `rsync`.
//!
//! ```text
//! <target>/flux-rsync/<host>/<user>/<profile>/
//!   FLUX-RECOVERY.txt          how to list, browse, restore and back up without Flux
//!   flux-backup.sh             the same backup with nothing but rsync and sh
//!   flux-repo.toml             marks the folder as Flux's
//!   latest -> snapshots/<id>   the newest snapshot
//!   snapshots/<id>/
//!     data/<absolute path>     the files, exactly as rsync wrote them
//!     rsync-command.txt        the exact command that wrote this snapshot
//!     rsync-filter.txt         the exclusion rules it used
//!     meta.json                Flux's notes (stats, name, pin); optional
//!   snapshots/.partial/        an unfinished snapshot; the next backup continues it
//! ```
//!
//! Each backup runs `rsync --archive --hard-links --relative --link-dest=../../<previous>/data`
//! into `.partial`, then renames it to its date and time. Unchanged files become hard
//! links to the previous snapshot, so every snapshot is complete but only changes take
//! space. Snapshots made by `flux-backup.sh` (without `meta.json`) are listed too.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use jiff::Timestamp;

use flux_core::profile::{ChangeDetection, Profile, SymlinkMode, Target};
use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_core::{Paths, ProfileId, sys, target_check};
use flux_fs::entry::{Entry, Kind};
use flux_fs::fsops::{Copier, XattrPolicy};
use flux_fs::outcome::{BackupOutcome, CheckSummary, PlanStats};
use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus, find_snapshot};
use flux_mirror::repo::RepoInfo;
use flux_transport::{SftpPool, SshTarget, shell_quote};

use crate::filter::RuleSet;
use crate::listing::{ListingCache, scan_local, scan_remote};
use crate::store::{Store, StoreLock};
use crate::{CompatError, MAX_STORED_ERRORS, kit, prune, tools};

pub const ROOT_DIR: &str = "flux-rsync";
pub const MARKER: &str = "flux-repo.toml";
pub const SNAPSHOTS: &str = "snapshots";
pub const PARTIAL: &str = ".partial";
pub const LATEST: &str = "latest";
pub const META: &str = "meta.json";
pub const COMMAND_FILE: &str = "rsync-command.txt";
pub const RULES_FILE: &str = "rsync-filter.txt";
const REPO_FORMAT: u32 = 1;
const SFTP_SESSIONS: usize = 4;

/// Repository root for a profile on a local target.
pub fn locate_local(target: &Path, host: &str, user: &str, profile: &str) -> PathBuf {
    target.join(ROOT_DIR).join(host).join(user).join(profile)
}

/// Repository root on an SSH server.
pub fn locate_remote(remote_path: &Path, host: &str, user: &str, profile: &str) -> String {
    let base = remote_path.display().to_string();
    let base = base.trim_end_matches('/');
    let base = if base.is_empty() { "." } else { base };
    format!("{base}/{ROOT_DIR}/{host}/{user}/{profile}")
}

/// The folder (local path or path on the server) a profile's backups go to.
pub fn repo_location(id: &ProfileId, profile: &Profile) -> String {
    let (host, user) = (sys::hostname(), sys::username());
    match &profile.target {
        Target::Local { path, .. } => locate_local(path, &host, &user, id.as_str())
            .display()
            .to_string(),
        Target::Ssh { path, .. } => locate_remote(path, &host, &user, id.as_str()),
    }
}

/// Where a profile's repository lives. Checks that a local target's drive is there.
/// `batch`: nobody can answer ssh prompts.
pub fn store_for(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    batch: bool,
) -> Result<Store, CompatError> {
    let (host, user) = (sys::hostname(), sys::username());
    match &profile.target {
        Target::Local { path, fs_uuid } => {
            target_check::check_local_target(path, fs_uuid.as_deref())?;
            Ok(Store::local(locate_local(path, &host, &user, id.as_str())))
        }
        Target::Ssh { path, .. } => {
            let t = SshTarget::from_target(&profile.target, &paths.runtime_dir, batch)
                .expect("ssh target");
            Ok(Store::remote(
                Arc::new(SftpPool::new(t, SFTP_SESSIONS)),
                locate_remote(path, &host, &user, id.as_str()),
            ))
        }
    }
}

fn snap_rel(id: &str) -> String {
    format!("{SNAPSHOTS}/{id}")
}

fn data_rel(id: &str) -> String {
    format!("{SNAPSHOTS}/{id}/data")
}

/// An rsync-engine repository.
pub struct RsyncRepo {
    store: Store,
    info: RepoInfo,
    cache: ListingCache,
    xattrs: XattrPolicy,
}

impl RsyncRepo {
    /// Open the repository of a profile. `CompatError::NoRepository` if there are no
    /// backups yet.
    pub fn open_profile(
        paths: &Paths,
        id: &ProfileId,
        profile: &Profile,
        batch: bool,
    ) -> Result<Self, CompatError> {
        let store = store_for(paths, id, profile, batch)?;
        Self::open(
            store,
            &paths.cache_dir,
            flux_fs::scan::xattr_policy(profile),
        )
    }

    pub fn open(store: Store, cache_dir: &Path, xattrs: XattrPolicy) -> Result<Self, CompatError> {
        let raw = match store.read(MARKER) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CompatError::NoRepository(store.describe()));
            }
            Err(e) => return Err(CompatError::remote(format!("{}: {e}", store.path(MARKER)))),
        };
        let info: RepoInfo = toml::from_str(&String::from_utf8_lossy(&raw))
            .map_err(|e| CompatError::Corrupt(format!("{}: {e}", store.path(MARKER))))?;
        if info.engine != "rsync" || info.format > REPO_FORMAT {
            return Err(CompatError::Foreign(store.describe(), "rsync"));
        }
        let cache = ListingCache::new(cache_dir, "rsync", &store.describe());
        let repo = Self {
            store,
            info,
            cache,
            xattrs,
        };
        Ok(repo)
    }

    /// Open, or create an empty repository.
    pub fn open_or_init(
        store: Store,
        cache_dir: &Path,
        xattrs: XattrPolicy,
        profile: &str,
    ) -> Result<Self, CompatError> {
        if !store.exists(MARKER) {
            store.mkdir_p(SNAPSHOTS)?;
            let info = RepoInfo {
                format: REPO_FORMAT,
                engine: "rsync".into(),
                profile: profile.into(),
                host: sys::hostname(),
                user: sys::username(),
                created: Timestamp::now(),
            };
            let text = format!(
                "# This folder holds backups made by Flux Backup in rsync compatibility mode.\n\
                 # Read FLUX-RECOVERY.txt to use them without Flux.\n{}",
                toml::to_string_pretty(&info).map_err(|e| CompatError::Corrupt(e.to_string()))?
            );
            store.write(MARKER, text.as_bytes(), 0o644)?;
        }
        Self::open(store, cache_dir, xattrs)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn describe(&self) -> String {
        self.store.describe()
    }

    /// Exclusive lock; also clears what interrupted deletions left behind.
    pub fn lock(&self) -> Result<StoreLock, CompatError> {
        let l = self.store.lock()?;
        if let Ok(names) = self.store.list(SNAPSHOTS) {
            for (n, _) in names {
                if n.starts_with(".deleting-") {
                    let _ = self.store.remove_tree(&format!("{SNAPSHOTS}/{n}"));
                }
            }
        }
        Ok(l)
    }

    /// Path of snapshot `id`'s folder, for people.
    pub fn snapshot_location(&self, id: &str) -> String {
        match &self.store {
            Store::Local { .. } => self.store.path(&snap_rel(id)),
            Store::Remote { .. } => format!("{}/{}", self.describe(), snap_rel(id)),
        }
    }

    /// Local folder holding snapshot `id`'s files (local repositories only).
    pub fn local_data_dir(&self, id: &str) -> Option<PathBuf> {
        self.store.local_path(&data_rel(id))
    }

    fn synthesize(&self, id: &str) -> SnapshotMeta {
        let time = crate::time_from_id(id)
            .or_else(|| self.store_mtime(&snap_rel(id)))
            .unwrap_or(Timestamp::UNIX_EPOCH);
        SnapshotMeta {
            format: 1,
            id: id.to_owned(),
            profile: self.info.profile.clone(),
            host: self.info.host.clone(),
            user: self.info.user.clone(),
            started: time,
            finished: time,
            status: SnapshotStatus::Complete,
            name: None,
            pinned: false,
            sources: Vec::new(),
            stats: SnapshotStats::default(),
            errors: Vec::new(),
            errors_total: 0,
            flux_version: String::new(),
        }
    }

    fn store_mtime(&self, rel: &str) -> Option<Timestamp> {
        match &self.store {
            Store::Local { .. } => {
                let m = std::fs::metadata(self.store.local_path(rel)?).ok()?;
                Timestamp::try_from(m.modified().ok()?).ok()
            }
            Store::Remote { pool, .. } => {
                let path = self.store.path(rel);
                let a = pool.with(|s| s.lstat(&path)).ok()?;
                Timestamp::from_second(i64::from(a.mtime?)).ok()
            }
        }
    }

    fn read_meta(&self, id: &str) -> Option<SnapshotMeta> {
        let raw = self.store.read(&format!("{}/{META}", snap_rel(id))).ok()?;
        match serde_json::from_slice::<SnapshotMeta>(&raw) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!("snapshot {id}: unreadable {META}: {e}");
                None
            }
        }
    }

    /// All snapshots, oldest first.
    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, CompatError> {
        let names = match self.store.list(SNAPSHOTS) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CompatError::remote(e)),
        };
        let mut out = Vec::new();
        for (name, is_dir) in names {
            if !is_dir || !crate::valid_snapshot_name(&name) {
                continue;
            }
            match self.read_meta(&name) {
                Some(mut m) => {
                    m.id = name;
                    out.push(m);
                }
                // Made without Flux (flux-backup.sh, or by hand).
                None if self.store.is_dir(&data_rel(&name)) => out.push(self.synthesize(&name)),
                None => {}
            }
        }
        // Names carry the start time to the second (`_2`, `_3` within one second), and
        // snapshots made without Flux only have their name; order by both.
        out.sort_by(|a, b| (a.started.as_second(), &a.id).cmp(&(b.started.as_second(), &b.id)));
        Ok(out)
    }

    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, CompatError> {
        Ok(find_snapshot(self.snapshots()?, spec)?)
    }

    /// The snapshot's entries, sorted by path (cached after the first listing).
    pub fn entries(&self, snap: &SnapshotMeta) -> Result<Vec<Entry>, CompatError> {
        let stamp = format!("{}:{}", snap.finished, snap.stats.entries);
        let id = &snap.id;
        self.cache
            .get(id, &stamp, || self.scan(&data_rel(id)))
            .map_err(|e| CompatError::Failed(format!("listing snapshot {id}: {e}")))
    }

    /// The entries of folder `rel` of the repository (uncached).
    fn scan(&self, rel: &str) -> std::io::Result<Vec<Entry>> {
        match &self.store {
            Store::Local { .. } => scan_local(
                &self.store.local_path(rel).expect("local store"),
                self.xattrs,
            ),
            Store::Remote { pool, .. } => scan_remote(pool.target(), &self.store.path(rel)),
        }
    }

    /// Give snapshots made without Flux (by `flux-backup.sh` or by hand) a `meta.json`
    /// with statistics from their files, so they show sizes and counts like the others.
    /// The caller holds the lock.
    pub fn adopt(&self, report: Reporter<'_>) {
        let Ok(snaps) = self.snapshots() else {
            return;
        };
        for s in snaps.iter().filter(|s| s.flux_version.is_empty()) {
            if self.store.exists(&format!("{}/{META}", snap_rel(&s.id))) {
                continue;
            }
            let Ok(entries) = self.entries(s) else {
                continue;
            };
            let mut m = s.clone();
            m.status = SnapshotStatus::Complete;
            let st = &mut m.stats;
            for e in &entries {
                st.entries += 1;
                match e.kind {
                    Kind::File => {
                        st.files += 1;
                        st.total_bytes += e.size;
                    }
                    Kind::Dir => st.dirs += 1,
                    Kind::Symlink => st.symlinks += 1,
                    _ => st.specials += 1,
                }
            }
            st.unchanged = st.files;
            let Ok(json) = serde_json::to_vec_pretty(&m) else {
                continue;
            };
            if self
                .store
                .write(&format!("{}/{META}", snap_rel(&s.id)), &json, 0o644)
                .is_ok()
            {
                report(Event::Info {
                    message: format!("found snapshot {} made without Flux", s.id),
                });
            }
        }
    }

    /// Change a snapshot's Flux notes (name, pin). Snapshots made without Flux get a
    /// `meta.json` the first time.
    pub fn update_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut SnapshotMeta),
    ) -> Result<SnapshotMeta, CompatError> {
        if !crate::valid_snapshot_name(id) || !self.store.is_dir(&snap_rel(id)) {
            return Err(CompatError::SnapshotNotFound(id.into()));
        }
        let mut m = self.read_meta(id).unwrap_or_else(|| self.synthesize(id));
        f(&mut m);
        let json =
            serde_json::to_vec_pretty(&m).map_err(|e| CompatError::Corrupt(e.to_string()))?;
        self.store
            .write(&format!("{}/{META}", snap_rel(id)), &json, 0o644)?;
        Ok(m)
    }

    /// Delete a snapshot (renamed away first, so an interruption never leaves a
    /// half-deleted snapshot that looks complete). The caller holds the lock.
    pub fn delete_snapshot(&self, id: &str) -> Result<(), CompatError> {
        if !crate::valid_snapshot_name(id) || !self.store.is_dir(&snap_rel(id)) {
            return Err(CompatError::SnapshotNotFound(id.into()));
        }
        let doomed = format!("{SNAPSHOTS}/.deleting-{id}");
        self.store.rename(&snap_rel(id), &doomed)?;
        self.store.remove_tree(&doomed)?;
        self.cache.forget(id);
        self.point_latest()
    }

    /// Point `latest` at the newest snapshot (or remove it when there are none).
    fn point_latest(&self) -> Result<(), CompatError> {
        let newest = self.snapshots()?.pop().map(|s| snap_rel(&s.id));
        self.store.set_link(LATEST, newest.as_deref())
    }

    fn data_file_rel(id: &str, e: &Entry) -> String {
        format!("{}/{}", data_rel(id), String::from_utf8_lossy(&e.path))
    }

    /// Copy the contents of `e` from snapshot `id` into `out`.
    pub fn copy_file(
        &self,
        id: &str,
        e: &Entry,
        out: &std::fs::File,
        copier: &Copier,
        progress: &Progress,
    ) -> std::io::Result<u64> {
        match self.local_data_dir(id) {
            Some(base) => copier.copy_to(&base.join(e.rel_path()), out, progress),
            None => self
                .store
                .copy_to(&Self::data_file_rel(id, e), out, copier, progress),
        }
    }

    /// Up to `len` bytes of `e` from snapshot `id` at `offset` (for mounting).
    pub fn read_range(
        &self,
        id: &str,
        e: &Entry,
        offset: u64,
        len: u64,
    ) -> std::io::Result<Vec<u8>> {
        match self.local_data_dir(id) {
            Some(base) => {
                use std::os::unix::fs::FileExt;
                let f = std::fs::File::open(base.join(e.rel_path()))?;
                let mut buf = vec![0u8; len.min(64 << 20) as usize];
                let mut done = 0;
                while done < buf.len() {
                    match f.read_at(&mut buf[done..], offset + done as u64) {
                        Ok(0) => break,
                        Ok(n) => done += n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e),
                    }
                }
                buf.truncate(done);
                Ok(buf)
            }
            None => self
                .store
                .read_range(&Self::data_file_rel(id, e), offset, len),
        }
    }
}

/// A snapshot as a restore source for the generic restore (same rules as every engine:
/// identical files skipped, differing ones kept aside or overwritten, symlink-safe).
pub struct RsyncSource<'a> {
    repo: &'a RsyncRepo,
    snap: SnapshotMeta,
}

impl<'a> RsyncSource<'a> {
    pub fn new(repo: &'a RsyncRepo, snap: &SnapshotMeta) -> Self {
        Self {
            repo,
            snap: snap.clone(),
        }
    }
}

impl flux_fs::restore::RestoreSource for RsyncSource<'_> {
    fn entries(&self) -> std::io::Result<Box<dyn Iterator<Item = std::io::Result<Entry>> + '_>> {
        let v = self.repo.entries(&self.snap)?;
        Ok(Box::new(v.into_iter().map(Ok)))
    }

    fn write_file(
        &self,
        e: &Entry,
        out: &std::fs::File,
        copier: &Copier,
        progress: &Progress,
    ) -> std::io::Result<u64> {
        self.repo.copy_file(&self.snap.id, e, out, copier, progress)
    }
}

/// How an rsync command is put together.
pub struct ArgSpec<'a> {
    pub profile: &'a Profile,
    /// The rules file (`--exclude-from`).
    pub rules_file: String,
    /// `--link-dest`, relative to the destination folder.
    pub link_dest: Option<String>,
    /// `-e` (the ssh command) for SSH targets.
    pub rsh: Option<String>,
    pub sources: Vec<String>,
    /// Destination folder (`path/` or `host:path/`).
    pub dest: String,
    pub dry_run: bool,
    /// Add the options Flux needs to follow a run (progress, statistics).
    pub live: bool,
}

/// The rsync arguments for a backup (without the program name).
pub fn rsync_args(spec: &ArgSpec<'_>) -> Vec<String> {
    let p = spec.profile;
    let mut a: Vec<String> = [
        "--archive",
        "--hard-links",
        "--numeric-ids",
        "--relative",
        "--delete",
        "--delete-excluded",
    ]
    .map(String::from)
    .to_vec();
    // In dry runs rsync can't read attributes of folders it hasn't created, and reports
    // that as errors; attributes don't change what would be copied.
    if p.options.preserve_acl && !spec.dry_run {
        a.push("--acls".into());
    }
    if p.options.preserve_xattr && !spec.dry_run {
        a.push("--xattrs".into());
    }
    if p.sources.one_file_system {
        a.push("--one-file-system".into());
    }
    if let Some(max) = p.sources.exclude_larger_than {
        a.push(format!("--max-size={}", max.bytes()));
    }
    match p.options.symlinks {
        SymlinkMode::Preserve => {}
        SymlinkMode::FollowUnsafe => a.push("--copy-unsafe-links".into()),
        SymlinkMode::Follow => a.push("--copy-links".into()),
    }
    if p.options.change_detection == ChangeDetection::Checksum {
        a.push("--checksum".into());
    }
    if let Target::Ssh {
        bandwidth_limit: Some(bw),
        ..
    } = &p.target
    {
        a.push(format!("--bwlimit={}", (bw.bytes() / 1024).max(1)));
    }
    a.push(format!("--exclude-from={}", spec.rules_file));
    if let Some(l) = &spec.link_dest {
        a.push(format!("--link-dest={l}"));
    }
    if let Some(rsh) = &spec.rsh {
        a.push("-e".into());
        a.push(rsh.clone());
    }
    if let Some(r) = &p.rsync {
        a.extend(r.extra_args.iter().cloned());
    }
    if spec.dry_run {
        a.push("--dry-run".into());
    }
    if spec.live {
        a.extend(["--info=progress2", "--stats", "--no-human-readable"].map(String::from));
    }
    a.extend(spec.sources.iter().cloned());
    a.push(spec.dest.clone());
    a
}

/// A command line for a POSIX shell.
pub fn shell_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A plain ssh command for people (no Flux connection sharing): `ssh -p 2222 -i key`.
pub fn plain_rsh(profile: &Profile) -> Option<String> {
    let Target::Ssh {
        port,
        identity_file,
        ssh_options,
        ..
    } = &profile.target
    else {
        return None;
    };
    let mut a = vec!["ssh".to_owned()];
    if let Some(p) = port {
        a.extend(["-p".into(), p.to_string()]);
    }
    if let Some(i) = identity_file {
        a.extend(["-i".into(), i.display().to_string()]);
    }
    for o in ssh_options {
        a.extend(["-o".into(), o.clone()]);
    }
    Some(
        a.iter()
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The source arguments: absolute paths, folders with a trailing `/` (so a symlinked
/// source folder is backed up as the folder it points to, as Flux's other engines do).
pub fn source_args(paths: &[PathBuf]) -> Vec<String> {
    flux_fs::scan::normalize_sources(paths)
        .into_iter()
        .map(|p| {
            let s = p.display().to_string();
            if p.is_dir() && s != "/" {
                format!("{s}/")
            } else {
                s
            }
        })
        .collect()
}

/// Folders that are never backed up: the repositories on a local target.
pub fn never_paths(profile: &Profile) -> Vec<PathBuf> {
    match &profile.target {
        Target::Local { path, .. } => vec![path.join(ROOT_DIR)],
        Target::Ssh { .. } => Vec::new(),
    }
}

/// `stats` with the counts and total size taken from what the snapshot holds (`listed`).
fn recount(stats: &RsyncStats, listed: &[Entry]) -> RsyncStats {
    let mut s = RsyncStats {
        files: listed.len() as u64,
        reg: 0,
        dirs: 0,
        links: 0,
        specials: 0,
        total_size: 0,
        ..stats.clone()
    };
    for e in listed {
        match e.kind {
            Kind::File => {
                s.reg += 1;
                s.total_size += e.size;
            }
            Kind::Dir => s.dirs += 1,
            Kind::Symlink => s.links += 1,
            _ => s.specials += 1,
        }
    }
    s
}

/// `rsync --stats` totals.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RsyncStats {
    pub files: u64,
    pub reg: u64,
    pub dirs: u64,
    pub links: u64,
    pub specials: u64,
    pub created: u64,
    pub deleted: u64,
    pub transferred: u64,
    pub total_size: u64,
    pub transferred_size: u64,
    /// The statistics block was seen at all.
    pub seen: bool,
}

fn number(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .unwrap_or("0")
        .replace([',', '.'], "")
        .parse()
        .unwrap_or(0)
}

impl RsyncStats {
    fn line(&mut self, line: &str) {
        let Some((key, value)) = line.split_once(':') else {
            return;
        };
        let (head, detail) = match value.split_once('(') {
            Some((h, d)) => (h, d.trim_end_matches(')')),
            None => (value, ""),
        };
        let n = number(head);
        match key.trim() {
            "Number of files" => {
                self.seen = true;
                self.files = n;
                for part in detail.split(", ") {
                    if let Some((k, v)) = part.split_once(':') {
                        let v = number(v);
                        match k.trim() {
                            "reg" => self.reg = v,
                            "dir" => self.dirs = v,
                            "link" => self.links = v,
                            "dev" | "special" => self.specials += v,
                            _ => {}
                        }
                    }
                }
            }
            "Number of created files" => self.created = n,
            "Number of deleted files" => self.deleted = n,
            "Number of regular files transferred" => self.transferred = n,
            "Total file size" => self.total_size = n,
            "Total transferred file size" => self.transferred_size = n,
            _ => {}
        }
    }
}

/// What one rsync run did.
pub struct RsyncRun {
    pub code: Option<i32>,
    pub stats: RsyncStats,
    pub errors: Vec<FileError>,
    /// Last lines of rsync's error output.
    pub stderr_tail: Vec<String>,
}

impl RsyncRun {
    fn failure(&self) -> String {
        format!(
            "rsync failed (exit {}): {}",
            self.code.map_or("signal".into(), |c| c.to_string()),
            self.stderr_tail.join("; ")
        )
    }
}

/// Run rsync, following `--info=progress2` and collecting `--stats` and per-file errors.
pub fn run_rsync(
    mut cmd: Command,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<RsyncRun, CompatError> {
    tracing::debug!("running {cmd:?}");
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CompatError::Tool(format!("cannot run rsync: {e}")))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let errors_thread = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    let mut stats = RsyncStats::default();
    let mut buf = Vec::new();
    let mut r = BufReader::new(stdout);
    let mut chunk = [0u8; 4096];
    let mut handle = |line: &[u8]| {
        let line = String::from_utf8_lossy(line);
        let t = line.trim_start();
        // "  1,234,567  45%  12.34MB/s  0:01:23 (xfr#12, to-chk=3/100)"
        if t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('%') {
            let mut f = t.split_whitespace();
            let done = f.next().map(number).unwrap_or(0);
            let pct = f
                .next()
                .and_then(|p| p.trim_end_matches('%').parse::<u64>().ok())
                .unwrap_or(0);
            progress.bytes_done.store(done, Relaxed);
            if let Some(total) = done.saturating_mul(100).checked_div(pct) {
                progress.bytes_total.store(total.max(done), Relaxed);
            }
            if let Some(chk) = t.split("to-chk=").nth(1)
                && let Some((left, total)) = chk.trim_end_matches(')').split_once('/')
                && let (Ok(l), Ok(n)) = (left.parse::<u64>(), total.parse::<u64>())
            {
                progress.entries_total.store(n, Relaxed);
                progress.entries_done.store(n.saturating_sub(l), Relaxed);
            }
        } else {
            stats.line(t);
        }
    };
    loop {
        if progress.is_cancelled() {
            let _ = child.kill();
            break;
        }
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                for &b in &chunk[..n] {
                    if b == b'\r' || b == b'\n' {
                        handle(&buf);
                        buf.clear();
                    } else {
                        buf.push(b);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if !buf.is_empty() {
        handle(&buf);
    }
    let status = child
        .wait()
        .map_err(|e| CompatError::Failed(e.to_string()))?;
    let stderr_lines = errors_thread.join().unwrap_or_default();
    if progress.is_cancelled() {
        return Err(CompatError::Cancelled);
    }
    let mut errors = Vec::new();
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
            errors.push(FileError { path, message });
        }
    }
    let code = status.code();
    if matches!(code, Some(23 | 24)) && errors.is_empty() {
        errors.push(FileError {
            path: String::new(),
            message: stderr_lines.join("; "),
        });
    }
    Ok(RsyncRun {
        code,
        stats,
        errors,
        stderr_tail: stderr_lines.iter().rev().take(5).rev().cloned().collect(),
    })
}

pub struct BackupRequest<'a> {
    pub paths: &'a Paths,
    pub id: &'a ProfileId,
    pub profile: &'a Profile,
    pub name: Option<String>,
    pub dry_run: bool,
    /// Apply retention after the backup.
    pub prune: bool,
    /// Nobody can answer ssh prompts.
    pub batch: bool,
}

/// Take a snapshot with rsync.
pub fn backup(
    req: &BackupRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, CompatError> {
    let started = Timestamp::now();
    let p = req.profile;
    progress.set_phase(Phase::Preparing);
    let rsync_version = tools::require_rsync()?;
    let store = store_for(req.paths, req.id, p, req.batch)?;
    if let Some(ssh) = store.ssh() {
        ssh.run("command -v rsync >/dev/null || { echo 'rsync is not installed on the server' >&2; exit 1; }")
            .map_err(CompatError::remote)?;
    }
    let xattrs = flux_fs::scan::xattr_policy(p);
    let repo = if req.dry_run {
        RsyncRepo::open(store, &req.paths.cache_dir, xattrs).ok()
    } else {
        Some(RsyncRepo::open_or_init(
            store,
            &req.paths.cache_dir,
            xattrs,
            req.id.as_str(),
        )?)
    };
    let _lock = match (&repo, req.dry_run) {
        (Some(r), false) => Some(r.lock()?),
        _ => None,
    };

    // Sources that exist.
    let mut source_paths = Vec::new();
    let mut missing_errors = Vec::new();
    for s in flux_fs::scan::normalize_sources(&p.sources.paths) {
        if s.symlink_metadata().is_ok() {
            source_paths.push(s);
        } else {
            report(Event::Warning {
                path: s.display().to_string(),
                message: "source does not exist; skipped".into(),
            });
            progress.warnings.fetch_add(1, Relaxed);
            missing_errors.push(FileError {
                path: s.display().to_string(),
                message: "source does not exist".into(),
            });
        }
    }
    if source_paths.is_empty() {
        return Err(CompatError::NoSources);
    }

    // Exclusion rules, including folders marked as caches.
    progress.set_phase(Phase::Scanning);
    let home = sys::home_dir();
    let never = never_paths(p);
    let mut rules = RuleSet::new(&p.sources, home.as_deref(), never.clone());
    if p.sources.exclude_caches || !p.sources.exclude_if_present.is_empty() {
        let filter = flux_core::filter::Filter::new(&p.sources, home.as_deref())?;
        rules.marked = crate::filter::find_marked(&p.sources, &filter, &never);
        if !rules.marked.is_empty() {
            report(Event::Info {
                message: format!(
                    "leaving out the contents of {} folders marked as caches or with a no-backup marker",
                    rules.marked.len()
                ),
            });
        }
    }
    if progress.is_cancelled() {
        return Err(CompatError::Cancelled);
    }

    if let (Some(r), false) = (&repo, req.dry_run) {
        r.adopt(report);
    }
    let prev = match &repo {
        Some(r) => r
            .snapshots()?
            .into_iter()
            .rev()
            .find(|s| r.store.is_dir(&data_rel(&s.id))),
        None => None,
    };
    let partial = format!("{SNAPSHOTS}/{PARTIAL}");
    let heading = format!(
        "Flux Backup: exclusion rules for a backup of \u{201c}{}\u{201d} ({}) started {}",
        p.name,
        req.id,
        local_time(started)
    );
    let rules_text = rules.render(&heading);

    // The rules file: inside the new snapshot for local targets; a local copy otherwise
    // (rsync reads it on this machine), uploaded next to the snapshot.
    let tmp_rules = tempfile_in(&req.paths.cache_dir)?;
    let rules_file = match (&repo, req.dry_run) {
        (Some(r), false) => {
            r.store.mkdir_p(&format!("{partial}/data"))?;
            r.store.write(
                &format!("{partial}/{RULES_FILE}"),
                rules_text.as_bytes(),
                0o644,
            )?;
            match r.store.local_path(&format!("{partial}/{RULES_FILE}")) {
                Some(local) => local.display().to_string(),
                None => write_tmp(&tmp_rules, &rules_text)?,
            }
        }
        _ => write_tmp(&tmp_rules, &rules_text)?,
    };

    let location = repo_location(req.id, p);
    let dest = match &p.target {
        Target::Ssh { .. } => {
            let ssh = SshTarget::from_target(&p.target, &req.paths.runtime_dir, req.batch)
                .expect("ssh target");
            format!("{}:{location}/{partial}/data/", ssh.destination())
        }
        Target::Local { .. } => format!("{location}/{partial}/data/"),
    };
    let rsh =
        SshTarget::from_target(&p.target, &req.paths.runtime_dir, req.batch).map(|t| t.rsync_rsh());
    let spec = ArgSpec {
        profile: p,
        rules_file,
        link_dest: prev.as_ref().map(|s| format!("../../{}/data", s.id)),
        rsh,
        sources: source_args(&source_paths),
        dest,
        dry_run: req.dry_run,
        live: true,
    };
    let args = rsync_args(&spec);
    let program = tools::rsync_program();
    let mut cmd = Command::new(&program);
    cmd.args(&args);
    progress.set_phase(Phase::Transferring);
    report(Event::Info {
        message: format!("running {}", shell_line(&program, &args)),
    });
    let run = run_rsync(cmd, progress, report);
    let _ = std::fs::remove_file(&tmp_rules);
    let run = run?;
    let partial_ok = matches!(run.code, Some(0 | 23 | 24));
    if !partial_ok {
        return Err(CompatError::Failed(run.failure()));
    }
    let mut errors = missing_errors;
    errors.extend(run.errors.iter().cloned());
    if !errors.is_empty() && !p.options.continue_on_errors && !req.dry_run {
        return Err(CompatError::Failed(format!(
            "{} items could not be backed up and continue_on_errors is off; the unfinished \
             snapshot is kept and the next backup continues it",
            errors.len()
        )));
    }
    let s = &run.stats;
    if req.dry_run {
        return Ok(BackupOutcome::DryRun {
            stats: PlanStats {
                entries: s.files,
                files: s.reg,
                dirs: s.dirs,
                symlinks: s.links,
                specials: s.specials,
                total_bytes: s.total_size,
                new: s.created,
                changed: s.transferred.saturating_sub(s.created),
                unchanged: s.reg.saturating_sub(s.transferred),
                deleted: 0,
                copy_files: s.transferred,
                copy_bytes: s.transferred_size,
                link_files: s.reg.saturating_sub(s.transferred),
            },
            errors,
        });
    }
    let repo = repo.expect("created above");

    progress.set_phase(Phase::Finalizing);
    // rsync counts files that `--max-size` skipped as if they were stored: count what the
    // snapshot really holds instead, and say how many were left out.
    let recounted;
    let s = match p.sources.exclude_larger_than {
        Some(max) => match repo.scan(&format!("{partial}/data")) {
            Ok(listed) => {
                recounted = recount(s, &listed);
                let left_out = s.reg.saturating_sub(recounted.reg);
                if left_out > 0 {
                    report(Event::Info {
                        message: if recounted.reg == 0 {
                            format!(
                                "no files were backed up: all {left_out} are larger than the \
                                 size limit ({max}); only folders were saved"
                            )
                        } else {
                            format!("left out {left_out} files larger than the size limit ({max})")
                        },
                    });
                }
                &recounted
            }
            Err(_) => s,
        },
        None => s,
    };
    let unchanged = prev.as_ref().is_some_and(|prev| {
        prev.stats.entries > 0
            && s.seen
            && s.created == 0
            && s.transferred == 0
            && s.deleted == 0
            && s.files == prev.stats.entries
            && errors.is_empty()
            && !p.options.snapshot_if_unchanged
    });
    if unchanged {
        let prev = prev.expect("checked");
        repo.store.remove_tree(&partial)?;
        let pruned = if req.prune {
            prune_locked(&repo, &p.retention, false, report)?.removed
        } else {
            Vec::new()
        };
        write_kit(&repo, req, &rsync_version, &prev.id, report);
        progress.set_phase(Phase::Done);
        return Ok(BackupOutcome::Unchanged {
            previous: prev.id,
            pruned,
        });
    }

    let id = crate::new_snapshot_id(started, |c| repo.store.exists(&snap_rel(c)));
    let finished = Timestamp::now();
    let deleted = prev
        .as_ref()
        .filter(|p| p.stats.entries > 0)
        .map_or(0, |prev| {
            prev.stats
                .entries
                .saturating_sub(s.files.saturating_sub(s.created))
        });
    let errors_total = errors.len() as u64;
    let mut stored_errors = errors;
    stored_errors.truncate(MAX_STORED_ERRORS);
    let meta = SnapshotMeta {
        format: 1,
        id: id.clone(),
        profile: req.id.to_string(),
        host: sys::hostname(),
        user: sys::username(),
        started,
        finished,
        status: if errors_total == 0 {
            SnapshotStatus::Complete
        } else {
            SnapshotStatus::CompleteWithErrors
        },
        name: req.name.clone(),
        pinned: false,
        sources: p.sources.paths.clone(),
        stats: SnapshotStats {
            entries: s.files,
            files: s.reg,
            dirs: s.dirs,
            symlinks: s.links,
            specials: s.specials,
            total_bytes: s.total_size,
            new: s.created,
            changed: s.transferred.saturating_sub(s.created),
            unchanged: s.reg.saturating_sub(s.transferred),
            deleted,
            copied_bytes: s.transferred_size,
            copied_files: s.transferred,
            linked_files: s.reg.saturating_sub(s.transferred),
            stored_bytes: s.transferred_size,
        },
        errors: stored_errors,
        errors_total,
        flux_version: flux_core::VERSION.into(),
    };
    let command_text = command_record(&meta, p, &rsync_version, &program, &args, &run);
    repo.store.write(
        &format!("{partial}/{COMMAND_FILE}"),
        command_text.as_bytes(),
        0o644,
    )?;
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| CompatError::Corrupt(e.to_string()))?;
    repo.store
        .write(&format!("{partial}/{META}"), &json, 0o644)?;
    repo.store.rename(&partial, &snap_rel(&id))?;
    repo.store.set_link(LATEST, Some(&snap_rel(&id)))?;

    let pruned = if req.prune {
        prune_locked(&repo, &p.retention, false, report)?.removed
    } else {
        Vec::new()
    };
    write_kit(&repo, req, &rsync_version, &meta.id, report);
    progress.set_phase(Phase::Done);
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned,
    })
}

fn local_time(t: Timestamp) -> String {
    t.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d %H:%M:%S %:z")
        .to_string()
}

/// `rsync-command.txt`: the exact command that wrote a snapshot, and how it went.
fn command_record(
    meta: &SnapshotMeta,
    profile: &Profile,
    version: &str,
    program: &str,
    args: &[String],
    run: &RsyncRun,
) -> String {
    let outcome = match run.code {
        Some(0) => "0 (success)".to_owned(),
        Some(23) => "23 (some files could not be copied; listed in meta.json)".to_owned(),
        Some(24) => "24 (some files vanished while they were copied)".to_owned(),
        Some(c) => c.to_string(),
        None => "killed by a signal".to_owned(),
    };
    format!(
        "# Flux Backup snapshot {id} of \u{201c}{name}\u{201d} ({profile_id})\n\
         # Written by rsync {version} on {host} for user {user}.\n\
         # Started:  {start}\n\
         # Finished: {end}\n\
         # rsync exit status: {outcome}\n\
         #\n\
         # Flux ran the command below with this folder named snapshots/{PARTIAL} and renamed\n\
         # it to {id} when rsync had finished. The exclusion rules it read are in\n\
         # {RULES_FILE}, next to this file. Files are under data/, at their original\n\
         # absolute paths. To restore with rsync alone, see ../../FLUX-RECOVERY.txt.\n\
         \n{line}\n",
        id = meta.id,
        name = profile.name,
        profile_id = meta.profile,
        host = meta.host,
        user = meta.user,
        start = local_time(meta.started),
        end = local_time(meta.finished),
        line = shell_line(program, args),
    )
}

fn write_kit(
    repo: &RsyncRepo,
    req: &BackupRequest<'_>,
    rsync_version: &str,
    newest: &str,
    report: Reporter<'_>,
) {
    let kit = kit::rsync_kit(req.id, req.profile, rsync_version, Some(newest));
    let r = repo
        .store
        .write(kit::RECOVERY_FILE, kit.recovery.as_bytes(), 0o644)
        .and_then(|()| {
            repo.store
                .write(kit::SCRIPT_FILE, kit.script.as_bytes(), 0o755)
        });
    if let Err(e) = r {
        report(Event::Warning {
            path: repo.store.path(kit::RECOVERY_FILE),
            message: format!("could not update the recovery kit: {e}"),
        });
    }
}

fn tempfile_in(dir: &Path) -> Result<PathBuf, CompatError> {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir).map_err(|e| CompatError::io(dir, e))?;
    Ok(dir.join(format!(
        ".flux-rsync-filter-{}-{}",
        std::process::id(),
        N.fetch_add(1, Relaxed)
    )))
}

fn write_tmp(path: &Path, text: &str) -> Result<String, CompatError> {
    std::fs::write(path, text).map_err(|e| CompatError::io(path, e))?;
    Ok(path.display().to_string())
}

/// `flux prune` for an rsync repository. The caller holds the lock.
pub fn prune_locked(
    repo: &RsyncRepo,
    policy: &flux_core::profile::Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<prune::PruneReport, CompatError> {
    let snaps = repo.snapshots()?;
    prune::apply(
        &snaps,
        policy,
        dry_run,
        &|| repo.store.free_space(),
        &mut |id| repo.delete_snapshot(id),
        report,
    )
}

/// Verify an rsync repository: every snapshot's folder can be listed, its file count
/// matches what rsync reported, and (`read_data`, local targets) every file reads back.
pub fn check(
    repo: &RsyncRepo,
    read_data: bool,
    progress: &Progress,
) -> Result<CheckSummary, CompatError> {
    progress.set_phase(Phase::Scanning);
    let snaps = repo.snapshots()?;
    let mut sum = CheckSummary {
        snapshots: snaps.len() as u64,
        ..Default::default()
    };
    if read_data && repo.store.is_remote() {
        sum.warnings.push(
            "reading every file back isn't supported over SSH; checked the listings only".into(),
        );
    }
    if repo.store.is_dir(&format!("{SNAPSHOTS}/{PARTIAL}")) {
        sum.warnings.push(
            "an unfinished backup is waiting in snapshots/.partial; the next backup continues it"
                .into(),
        );
    }
    let mut lists = Vec::new();
    for s in &snaps {
        if progress.is_cancelled() {
            return Err(CompatError::Cancelled);
        }
        match repo.entries(s) {
            Ok(v) => {
                if read_data && !repo.store.is_remote() {
                    progress.bytes_total.fetch_add(
                        v.iter()
                            .filter(|e| e.kind == Kind::File)
                            .map(|e| e.size)
                            .sum(),
                        Relaxed,
                    );
                }
                lists.push((s, v));
            }
            Err(e) => sum.errors.push(format!("snapshot {}: {e}", s.id)),
        }
    }
    if read_data && !repo.store.is_remote() {
        progress.set_phase(Phase::Transferring);
        let mut buf = vec![0u8; 1 << 20];
        for (s, v) in &lists {
            let base = repo.local_data_dir(&s.id).expect("local");
            for e in v.iter().filter(|e| e.kind == Kind::File) {
                if progress.is_cancelled() {
                    return Err(CompatError::Cancelled);
                }
                let path = base.join(e.rel_path());
                let res = std::fs::File::open(&path).and_then(|mut f| {
                    let mut n = 0u64;
                    loop {
                        let k = f.read(&mut buf)?;
                        if k == 0 {
                            break Ok(n);
                        }
                        n += k as u64;
                        progress.bytes_done.fetch_add(k as u64, Relaxed);
                    }
                });
                match res {
                    Ok(n) if n == e.size => {}
                    Ok(n) => sum.errors.push(format!(
                        "snapshot {}: /{}: read {n} bytes, expected {}",
                        s.id,
                        String::from_utf8_lossy(&e.path),
                        e.size
                    )),
                    Err(err) => sum.errors.push(format!(
                        "snapshot {}: /{}: {err}",
                        s.id,
                        String::from_utf8_lossy(&e.path)
                    )),
                }
                sum.bytes_read += e.size;
                sum.checked += 1;
            }
        }
    } else {
        sum.checked = lists
            .iter()
            .map(|(_, v)| v.iter().filter(|e| e.kind == Kind::File).count() as u64)
            .sum();
    }
    progress.set_phase(Phase::Done);
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stats() {
        let mut s = RsyncStats::default();
        for l in [
            "Number of files: 1,234 (reg: 1,000, dir: 200, link: 30, dev: 2, special: 2)",
            "Number of created files: 5 (reg: 5)",
            "Number of deleted files: 0",
            "Number of regular files transferred: 7",
            "Total file size: 123,456 bytes",
            "Total transferred file size: 789 bytes",
        ] {
            s.line(l);
        }
        assert_eq!(
            s,
            RsyncStats {
                files: 1234,
                reg: 1000,
                dirs: 200,
                links: 30,
                specials: 4,
                created: 5,
                deleted: 0,
                transferred: 7,
                total_size: 123_456,
                transferred_size: 789,
                seen: true,
            }
        );
    }

    #[test]
    fn builds_commands() {
        let mut p = Profile::new(
            "Home",
            Target::Local {
                path: "/mnt/b".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        );
        p.engine = flux_core::Engine::Rsync;
        p.options.preserve_acl = false;
        p.sources.exclude_larger_than = Some(flux_core::ByteSize(1 << 20));
        p.options.symlinks = SymlinkMode::FollowUnsafe;
        p.rsync = Some(flux_core::profile::RsyncOptions {
            extra_args: vec!["--sparse".into()],
        });
        let a = rsync_args(&ArgSpec {
            profile: &p,
            rules_file: "/tmp/r".into(),
            link_dest: Some("../../x/data".into()),
            rsh: None,
            sources: vec!["/home/me/".into()],
            dest: "/mnt/b/s/".into(),
            dry_run: false,
            live: false,
        });
        let line = shell_line("rsync", &a);
        assert_eq!(
            line,
            "rsync --archive --hard-links --numeric-ids --relative --delete --delete-excluded \
             --xattrs --max-size=1048576 --copy-unsafe-links --exclude-from=/tmp/r \
             --link-dest=../../x/data --sparse /home/me/ /mnt/b/s/"
        );
    }
}
