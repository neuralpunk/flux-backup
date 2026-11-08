//! The Borg engine: a standard Borg 1.x repository written by the system `borg`.
//!
//! ```text
//! <target>/flux-borg/<host>/<user>/<profile>/
//!   FLUX-RECOVERY.txt     how to list, browse, restore and back up with borg alone
//!   flux-backup.sh        the same backup with nothing but borg and sh
//!   flux-patterns.txt     the exclusion patterns (borg --patterns-from syntax)
//!   repo/                 the Borg repository (borg init --encryption=repokey-blake2)
//! ```
//!
//! Every snapshot is an archive named after its start time (`2026-11-07_18-30-00`). What
//! Flux adds (a snapshot's name, its pin) is kept in the archive's comment as JSON
//! (`{"flux":1,"pinned":true}`), so it lives in the repository, not on this machine.
//! Statistics and listings are cached on this machine (archives never change); losing the
//! cache loses nothing. Every backup makes an archive, even when nothing changed (that's
//! what borg does, and deduplication makes it nearly free).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use flux_core::profile::{BorgOptions, ChangeDetection, Profile, Target};
use flux_core::progress::{Event, Phase, Progress, Reporter};
use flux_core::{Paths, ProfileId, sys, target_check};
use flux_fs::entry::{Entry, Kind};
use flux_fs::fsops::{Copier, XattrPolicy};
use flux_fs::outcome::{BackupOutcome, CheckSummary, PlanStats};
use flux_fs::snapshot::{FileError, SnapshotMeta, SnapshotStats, SnapshotStatus, find_snapshot};
use flux_transport::{SftpPool, SshTarget, shell_quote};

use crate::listing::{ListingCache, fnv, scan_local};
use crate::store::Store;
use crate::{CompatError, MAX_STORED_ERRORS, kit, prune, tools};

pub const ROOT_DIR: &str = "flux-borg";
pub const REPO_DIR: &str = "repo";
pub const PATTERNS_FILE: &str = "flux-patterns.txt";
const SFTP_SESSIONS: usize = 2;
/// Seconds to wait for Borg's repository lock in commands that change the repository.
const LOCK_WAIT: &str = "30";

/// The folder holding a profile's repository and recovery kit, on a local target.
pub fn locate_local(target: &Path, host: &str, user: &str, profile: &str) -> PathBuf {
    target.join(ROOT_DIR).join(host).join(user).join(profile)
}

/// The same folder on an SSH server.
pub fn locate_remote(remote_path: &Path, host: &str, user: &str, profile: &str) -> String {
    let base = remote_path.display().to_string();
    let base = base.trim_end_matches('/');
    let base = if base.is_empty() { "." } else { base };
    format!("{base}/{ROOT_DIR}/{host}/{user}/{profile}")
}

/// The folder (local path, or path on the server) holding a profile's repository.
pub fn folder_location(id: &ProfileId, profile: &Profile) -> String {
    let (host, user) = (sys::hostname(), sys::username());
    match &profile.target {
        Target::Local { path, .. } => locate_local(path, &host, &user, id.as_str())
            .display()
            .to_string(),
        Target::Ssh { path, .. } => locate_remote(path, &host, &user, id.as_str()),
    }
}

/// The repository as borg names it: a local path or `ssh://user@host:port/path`.
/// `with_port`: put the port in the URL (for people); Flux's own runs pass it to ssh.
pub fn repo_url(id: &ProfileId, profile: &Profile, with_port: bool) -> String {
    let folder = folder_location(id, profile);
    match &profile.target {
        Target::Local { .. } => format!("{folder}/{REPO_DIR}"),
        Target::Ssh {
            host, user, port, ..
        } => {
            let path = if folder.starts_with('/') {
                folder
            } else {
                format!("/./{folder}")
            };
            format!(
                "ssh://{}{host}{}{path}/{REPO_DIR}",
                user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
                match port {
                    Some(p) if with_port => format!(":{p}"),
                    _ => String::new(),
                }
            )
        }
    }
}

/// Flux's notes in an archive's comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FluxComment {
    pub flux: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

impl FluxComment {
    fn parse(comment: &str) -> Self {
        serde_json::from_str(comment).unwrap_or_default()
    }

    /// For `--comment`, which expands `{placeholders}`: braces are doubled.
    fn to_arg(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_default()
            .replace('{', "{{")
            .replace('}', "}}")
    }
}

/// What Flux remembers about an archive on this machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cached {
    stats: SnapshotStats,
    status: Option<SnapshotStatus>,
    errors: Vec<FileError>,
    errors_total: u64,
    sources: Vec<PathBuf>,
}

/// One message from `--log-json`.
#[derive(Debug, Clone)]
struct LogMessage {
    level: String,
    msgid: String,
    message: String,
}

/// What one borg run did.
struct BorgRun {
    code: Option<i32>,
    stdout: Vec<u8>,
    messages: Vec<LogMessage>,
}

impl BorgRun {
    fn errors(&self) -> Vec<&LogMessage> {
        self.messages
            .iter()
            .filter(|m| m.level == "ERROR" || m.level == "CRITICAL")
            .collect()
    }

    /// The run's failure as an error, or `Ok` for exit status 0 or 1 (warnings).
    fn ok(self, what: &str) -> Result<Self, CompatError> {
        if matches!(self.code, Some(0 | 1)) {
            return Ok(self);
        }
        for m in &self.messages {
            match m.msgid.as_str() {
                "PassphraseWrong" => return Err(CompatError::WrongPassphrase),
                "Repository.DoesNotExist" | "Repository.InvalidRepository" => {
                    return Err(CompatError::NoRepository(m.message.clone()));
                }
                "LockTimeout" | "LockFailed" | "LockError" => {
                    return Err(CompatError::Locked(m.message.clone()));
                }
                _ if m.message.contains("passphrase") || m.message.contains("Passphrase") => {
                    return Err(CompatError::PassphraseRequired);
                }
                _ => {}
            }
        }
        let errors = self.errors();
        let detail = if errors.is_empty() {
            self.messages
                .iter()
                .rev()
                .take(3)
                .map(|m| m.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            errors
                .iter()
                .map(|m| m.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        };
        Err(CompatError::Failed(format!(
            "borg {what} failed (exit {}): {detail}",
            self.code.map_or("signal".into(), |c| c.to_string())
        )))
    }
}

/// A profile's Borg repository.
pub struct BorgRepo {
    /// The folder holding the repository and the recovery kit.
    store: Store,
    url: String,
    rsh: Option<String>,
    remote_path: Option<String>,
    passphrase: Option<Zeroizing<String>>,
    profile_id: String,
    cache: ListingCache,
    cache_dir: PathBuf,
}

impl BorgRepo {
    /// The repository of a profile (nothing is read yet). `passphrase`: for encrypted
    /// repositories. `batch`: nobody can answer ssh prompts.
    pub fn for_profile(
        paths: &Paths,
        id: &ProfileId,
        profile: &Profile,
        passphrase: Option<Zeroizing<String>>,
        batch: bool,
    ) -> Result<Self, CompatError> {
        let (host, user) = (sys::hostname(), sys::username());
        let opts = profile.borg.clone().unwrap_or_default();
        let (store, rsh) = match &profile.target {
            Target::Local { path, fs_uuid } => {
                target_check::check_local_target(path, fs_uuid.as_deref())?;
                (
                    Store::local(locate_local(path, &host, &user, id.as_str())),
                    None,
                )
            }
            Target::Ssh { path, .. } => {
                let t = SshTarget::from_target(&profile.target, &paths.runtime_dir, batch)
                    .expect("ssh target");
                let rsh = t.rsync_rsh();
                (
                    Store::remote(
                        Arc::new(SftpPool::new(t, SFTP_SESSIONS)),
                        locate_remote(path, &host, &user, id.as_str()),
                    ),
                    Some(rsh),
                )
            }
        };
        let url = repo_url(id, profile, false);
        let key = format!("{}|{url}", store.describe());
        Ok(Self {
            store,
            cache: ListingCache::new(&paths.cache_dir, "borg", &key),
            cache_dir: paths
                .cache_dir
                .join("compat")
                .join("borg")
                .join(fnv(key.as_bytes())),
            url,
            rsh,
            remote_path: opts.remote_path,
            passphrase,
            profile_id: id.to_string(),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Where restores in place stage what they extract (Flux's cache, on this machine).
    pub fn staging_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn describe(&self) -> String {
        match &self.store {
            Store::Local { .. } => self.url.clone(),
            Store::Remote { .. } => format!("{}/{REPO_DIR}", self.store.describe()),
        }
    }

    /// Is there a repository yet?
    pub fn exists(&self) -> bool {
        self.store.exists(&format!("{REPO_DIR}/config"))
    }

    fn command(&self) -> Command {
        let mut c = Command::new(tools::borg_program());
        c.env("BORG_RELOCATED_REPO_ACCESS_IS_OK", "yes")
            .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
            .env("BORG_DISPLAY_PASSPHRASE", "no")
            .env_remove("BORG_EXIT_CODES")
            .env_remove("BORG_REPO")
            .stdin(Stdio::null());
        if let Some(rsh) = &self.rsh {
            c.env("BORG_RSH", rsh);
        }
        if let Some(r) = &self.remote_path {
            c.env("BORG_REMOTE_PATH", r);
        }
        match &self.passphrase {
            Some(p) => {
                c.env("BORG_PASSPHRASE", p.as_str())
                    .env_remove("BORG_PASSCOMMAND")
                    .env_remove("BORG_PASSPHRASE_FD");
            }
            None => {
                c.env_remove("BORG_PASSPHRASE");
            }
        }
        c
    }

    /// Run borg with `--log-json`, following its progress and file messages.
    fn run(
        &self,
        args: &[String],
        cwd: Option<&Path>,
        progress: Option<&Progress>,
        on_json: &mut dyn FnMut(&Value),
    ) -> Result<BorgRun, CompatError> {
        let mut cmd = self.command();
        cmd.arg("--log-json").args(args);
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        // Never log the environment: it holds the passphrase.
        tracing::debug!(
            "running borg {}",
            args.iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| CompatError::Tool(format!("cannot run borg: {e}")))?;
        let mut stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let out_thread = std::thread::spawn(move || {
            let mut v = Vec::new();
            let _ = stdout.read_to_end(&mut v);
            v
        });
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let err_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut messages = Vec::new();
        let mut handle = |line: String| match serde_json::from_str::<Value>(&line) {
            Ok(v) => {
                if v["type"] == "log_message" {
                    messages.push(LogMessage {
                        level: v["levelname"].as_str().unwrap_or("").to_owned(),
                        msgid: v["msgid"].as_str().unwrap_or("").to_owned(),
                        message: v["message"].as_str().unwrap_or("").to_owned(),
                    });
                }
                on_json(&v);
            }
            Err(_) if !line.trim().is_empty() => messages.push(LogMessage {
                level: "ERROR".into(),
                msgid: String::new(),
                message: line,
            }),
            Err(_) => {}
        };
        let mut interrupted = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => handle(line),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if !interrupted && progress.is_some_and(|p| p.is_cancelled()) {
                // Like Ctrl-C: borg stops cleanly and releases its lock.
                if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::INT);
                }
                interrupted = true;
            }
        }
        let _ = err_thread.join();
        let status = child
            .wait()
            .map_err(|e| CompatError::Failed(e.to_string()))?;
        let stdout = out_thread.join().unwrap_or_default();
        if interrupted {
            return Err(CompatError::Cancelled);
        }
        Ok(BorgRun {
            code: status.code(),
            stdout,
            messages,
        })
    }

    fn run_simple(&self, args: &[String], what: &str) -> Result<BorgRun, CompatError> {
        self.run(args, None, None, &mut |_| {})?.ok(what)
    }

    fn arg(&self, archive: Option<&str>) -> String {
        match archive {
            Some(a) => format!("{}::{a}", self.url),
            None => self.url.clone(),
        }
    }

    /// Create the repository.
    pub fn init(&self, encryption: flux_core::profile::BorgEncryption) -> Result<(), CompatError> {
        self.store.mkdir_p("")?;
        self.run_simple(
            &[
                "init".into(),
                format!("--encryption={}", encryption.as_arg()),
                "--make-parent-dirs".into(),
                self.arg(None),
            ],
            "init",
        )?;
        Ok(())
    }

    fn cache_path(&self, name: &str) -> PathBuf {
        self.cache_dir.join(name)
    }

    fn load_cached(&self) -> BTreeMap<String, Cached> {
        std::fs::read(self.cache_path("archives-stats.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_cached(&self, m: &BTreeMap<String, Cached>) {
        if let Ok(json) = serde_json::to_vec(m) {
            let _ = std::fs::create_dir_all(&self.cache_dir);
            let _ = flux_core::store::write_atomic(
                &self.cache_path("archives-stats.json"),
                &json,
                0o600,
            );
        }
    }

    fn remember(&self, id: &str, c: Cached) {
        let mut m = self.load_cached();
        m.insert(id.to_owned(), c);
        self.save_cached(&m);
    }

    /// The archives, oldest first. While another borg holds the repository (a backup is
    /// running), the last list seen is returned.
    /// Check that the passphrase opens the repository: always asks borg (never the cached
    /// listing [`BorgRepo::snapshots`] may use).
    pub fn verify(&self) -> Result<(), CompatError> {
        self.run_simple(
            &[
                "list".into(),
                "--short".into(),
                "--last".into(),
                "1".into(),
                self.arg(None),
            ],
            "list",
        )
        .map(|_| ())
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotMeta>, CompatError> {
        // A local repository this process already listed with this passphrase, unchanged
        // since: reuse that listing rather than start borg (half a second or more) for
        // every view of the app. In memory only, so a wrong passphrase is never answered
        // from it, and the command line (one listing per process) always asks borg.
        let marker = self.transaction_marker();
        let tag = listed::tag(self.passphrase.as_deref().map(|p| p.as_str()));
        if let Some(m) = &marker
            && let Some(raw) = listed::get(&self.url, m, &tag)
            && let Ok(out) = self.parse_archives(&raw)
        {
            return Ok(out);
        }
        let listed = self.run_simple(
            &[
                "list".into(),
                "--json".into(),
                "--format".into(),
                "{name}{comment}{start}{end}{hostname}{username}".into(),
                self.arg(None),
            ],
            "list",
        );
        let raw = match listed {
            Ok(r) => {
                let _ = std::fs::create_dir_all(&self.cache_dir);
                let _ = flux_core::store::write_atomic(
                    &self.cache_path("archives.json"),
                    &r.stdout,
                    0o600,
                );
                // The marker read before listing: if borg wrote meanwhile, the next call
                // sees a different marker and lists again.
                if let Some(m) = &marker {
                    listed::put(&self.url, m, &tag, &r.stdout);
                }
                r.stdout
            }
            Err(CompatError::Locked(why)) => {
                tracing::warn!("borg list: {why}; showing the archives seen last");
                std::fs::read(self.cache_path("archives.json"))
                    .map_err(|_| CompatError::Locked(why))?
            }
            Err(e) => return Err(e),
        };
        self.parse_archives(&raw)
    }

    /// Borg's newest transaction in a local repository (its `index.N` file): it changes
    /// with every write (backup, delete, prune, compact), so an unchanged marker means an
    /// unchanged archive list. `None` for remote repositories, or when it can't be read.
    fn transaction_marker(&self) -> Option<String> {
        let Store::Local { root } = &self.store else {
            return None;
        };
        let dir = root.join(REPO_DIR);
        let newest = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_prefix("index.")?
                    .parse::<u64>()
                    .ok()
            })
            .max()?;
        let mtime = std::fs::metadata(dir.join(format!("index.{newest}")))
            .and_then(|m| m.modified())
            .ok()?;
        Some(format!("{newest} {mtime:?}"))
    }

    fn parse_archives(&self, raw: &[u8]) -> Result<Vec<SnapshotMeta>, CompatError> {
        let v: Value = serde_json::from_slice(raw)
            .map_err(|e| CompatError::Failed(format!("unexpected output from borg list: {e}")))?;
        let cached = self.load_cached();
        let mut out: Vec<SnapshotMeta> = v["archives"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| self.meta_of(a, &cached))
            .collect();
        out.sort_by(|a, b| (a.started.as_second(), &a.id).cmp(&(b.started.as_second(), &b.id)));
        Ok(out)
    }

    fn meta_of(&self, a: &Value, cached: &BTreeMap<String, Cached>) -> Option<SnapshotMeta> {
        let name = a["name"].as_str()?.to_owned();
        let started = parse_time(a["start"].as_str()?)?;
        let finished = a["end"].as_str().and_then(parse_time).unwrap_or(started);
        let comment = FluxComment::parse(a["comment"].as_str().unwrap_or(""));
        let c = cached.get(&name).cloned().unwrap_or_default();
        Some(SnapshotMeta {
            format: 1,
            id: name,
            profile: self.profile_id.clone(),
            host: a["hostname"].as_str().unwrap_or("").to_owned(),
            user: a["username"].as_str().unwrap_or("").to_owned(),
            started,
            finished,
            status: c.status.unwrap_or(SnapshotStatus::Complete),
            name: comment.name,
            pinned: comment.pinned,
            sources: c.sources,
            stats: c.stats,
            errors: c.errors,
            errors_total: c.errors_total,
            flux_version: if comment.flux > 0 {
                flux_core::VERSION.into()
            } else {
                String::new()
            },
        })
    }

    pub fn find(&self, spec: &str) -> Result<SnapshotMeta, CompatError> {
        Ok(find_snapshot(self.snapshots()?, spec)?)
    }

    /// An archive's entries, sorted by path (cached after the first listing).
    pub fn entries(&self, snap: &SnapshotMeta) -> Result<Vec<Entry>, CompatError> {
        let stamp = snap.started.to_string();
        self.cache
            .get(&snap.id, &stamp, || {
                let r = self
                    .run_simple(
                        &[
                            "list".into(),
                            "--json-lines".into(),
                            self.arg(Some(&snap.id)),
                        ],
                        "list",
                    )
                    .map_err(std::io::Error::other)?;
                parse_listing(&r.stdout).map_err(std::io::Error::other)
            })
            .map_err(|e| match e.downcast::<CompatError>() {
                Ok(c) => c,
                Err(e) => CompatError::Failed(format!("listing archive {}: {e}", snap.id)),
            })
    }

    /// Change an archive's name or pin (kept in its comment).
    pub fn update_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut SnapshotMeta),
    ) -> Result<SnapshotMeta, CompatError> {
        let mut m = self.find(id)?;
        if m.id != id {
            return Err(CompatError::SnapshotNotFound(id.into()));
        }
        f(&mut m);
        let comment = FluxComment {
            flux: 1,
            name: m.name.clone(),
            pinned: m.pinned,
        };
        self.run_simple(
            &[
                "recreate".into(),
                "--lock-wait".into(),
                LOCK_WAIT.into(),
                "--comment".into(),
                comment.to_arg(),
                self.arg(Some(id)),
            ],
            "recreate",
        )?;
        Ok(m)
    }

    /// Delete archives, then free their space (`borg compact`). Returns the bytes freed,
    /// as borg reports them.
    pub fn delete(&self, ids: &[&str], report: Reporter<'_>) -> Result<Option<u64>, CompatError> {
        if ids.is_empty() {
            return Ok(Some(0));
        }
        let names = self
            .snapshots()?
            .into_iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        for id in ids {
            if !names.iter().any(|n| n == id) {
                return Err(CompatError::SnapshotNotFound((*id).into()));
            }
        }
        let mut args = vec![
            "delete".into(),
            "--lock-wait".into(),
            LOCK_WAIT.into(),
            self.arg(None),
        ];
        args.extend(ids.iter().map(|s| s.to_string()));
        self.run_simple(&args, "delete")?;
        for id in ids {
            self.cache.forget(id);
            report(Event::Info {
                message: format!("deleted archive {id}"),
            });
        }
        let mut m = self.load_cached();
        for id in ids {
            m.remove(*id);
        }
        self.save_cached(&m);
        self.compact()
    }

    /// `borg compact`: free the space of deleted archives.
    pub fn compact(&self) -> Result<Option<u64>, CompatError> {
        let r = self.run_simple(
            &[
                "compact".into(),
                "--verbose".into(),
                "--lock-wait".into(),
                LOCK_WAIT.into(),
                self.arg(None),
            ],
            "compact",
        )?;
        Ok(r.messages.iter().find_map(|m| parse_freed(&m.message)))
    }

    /// Extract `paths` (absolute) of archive `id` into folder `dest`.
    pub fn extract(
        &self,
        id: &str,
        paths: &[PathBuf],
        dest: &Path,
        progress: &Progress,
    ) -> Result<Vec<String>, CompatError> {
        let mut args = vec!["extract".into(), "--progress".into(), self.arg(Some(id))];
        args.extend(paths.iter().map(|p| {
            p.display()
                .to_string()
                .trim_start_matches('/')
                .trim_end_matches('/')
                .to_owned()
        }));
        let r = self.run(&args, Some(dest), Some(progress), &mut |v| {
            if v["type"] == "progress_percent" && v["msgid"] == "extract" {
                if let Some(t) = v["total"].as_u64() {
                    progress.bytes_total.store(t, Relaxed);
                }
                if let Some(c) = v["current"].as_u64() {
                    progress.bytes_done.store(c, Relaxed);
                }
            }
        })?;
        let warnings = r
            .messages
            .iter()
            .filter(|m| m.level == "WARNING")
            .map(|m| m.message.clone())
            .collect();
        r.ok("extract")?;
        Ok(warnings)
    }

    /// A temporary file holding the contents of `e` from archive `id` (for mounting).
    pub fn extract_file(&self, id: &str, e: &Entry) -> std::io::Result<std::fs::File> {
        std::fs::create_dir_all(&self.cache_dir)?;
        let mut cmd = self.command();
        cmd.args(["extract", "--stdout"])
            .arg(self.arg(Some(id)))
            .arg(std::ffi::OsStr::new(
                std::str::from_utf8(&e.path).map_err(std::io::Error::other)?,
            ))
            .stderr(Stdio::null());
        let tmp = self
            .cache_dir
            .join(format!(".extract-{}-{}", std::process::id(), fnv(&e.path)));
        let out = std::fs::File::create(&tmp)?;
        let status = cmd.stdout(out).status()?;
        let f = std::fs::File::open(&tmp);
        let _ = std::fs::remove_file(&tmp);
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "borg extract failed ({status})"
            )));
        }
        f
    }

    /// Fill in statistics for archives made without Flux (`borg info`, once each).
    pub fn adopt(&self, snaps: &[SnapshotMeta], report: Reporter<'_>) {
        let mut cached = self.load_cached();
        let mut changed = false;
        let missing: Vec<&SnapshotMeta> = snaps
            .iter()
            .filter(|s| !cached.contains_key(&s.id))
            .collect();
        for s in missing {
            let Ok(r) = self.run_simple(
                &["info".into(), "--json".into(), self.arg(Some(&s.id))],
                "info",
            ) else {
                continue;
            };
            let Ok(v) = serde_json::from_slice::<Value>(&r.stdout) else {
                continue;
            };
            let st = &v["archives"][0]["stats"];
            let files = st["nfiles"].as_u64().unwrap_or(0);
            cached.insert(
                s.id.clone(),
                Cached {
                    stats: SnapshotStats {
                        entries: files,
                        files,
                        total_bytes: st["original_size"].as_u64().unwrap_or(0),
                        unchanged: files,
                        stored_bytes: st["deduplicated_size"].as_u64().unwrap_or(0),
                        ..Default::default()
                    },
                    status: Some(SnapshotStatus::Complete),
                    ..Default::default()
                },
            );
            changed = true;
            if s.flux_version.is_empty() {
                report(Event::Info {
                    message: format!("found archive {} made without Flux", s.id),
                });
            }
        }
        if changed {
            self.save_cached(&cached);
        }
    }
}

/// "compaction freed about 9.94 kB repository space." → bytes
fn parse_freed(message: &str) -> Option<u64> {
    let rest = message.split("freed about ").nth(1)?;
    let mut w = rest.split_whitespace();
    let n: f64 = w.next()?.parse().ok()?;
    let mult = match w.next()? {
        "B" => 1.0,
        "kB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// Borg's local, timezone-less timestamps.
fn parse_time(s: &str) -> Option<Timestamp> {
    let dt: jiff::civil::DateTime = s.parse().ok()?;
    dt.to_zoned(jiff::tz::TimeZone::system())
        .ok()
        .map(|z| z.timestamp())
}

/// `-rwsr-xr-x` → 0o4755
fn parse_mode(m: &str) -> u32 {
    let b = m.as_bytes();
    if b.len() < 10 {
        return 0o644;
    }
    let mut mode = 0;
    let bit = |i: usize, c: u8, v: u32| if b[i] == c { v } else { 0 };
    mode |= bit(1, b'r', 0o400) | bit(2, b'w', 0o200);
    mode |= match b[3] {
        b'x' => 0o100,
        b's' => 0o4100,
        b'S' => 0o4000,
        _ => 0,
    };
    mode |= bit(4, b'r', 0o40) | bit(5, b'w', 0o20);
    mode |= match b[6] {
        b'x' => 0o10,
        b's' => 0o2010,
        b'S' => 0o2000,
        _ => 0,
    };
    mode |= bit(7, b'r', 0o4) | bit(8, b'w', 0o2);
    mode |= match b[9] {
        b'x' => 0o1,
        b't' => 0o1001,
        b'T' => 0o1000,
        _ => 0,
    };
    mode
}

/// `borg list --json-lines` output as entries, sorted by path. Hard links (borg marks
/// them with an `h` mode and names the first path in `source`) share an inode number.
pub fn parse_listing(raw: &[u8]) -> Result<Vec<Entry>, CompatError> {
    let mut out = Vec::new();
    let mut sizes: HashMap<Vec<u8>, u64> = HashMap::new();
    for line in raw.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let v: Value = serde_json::from_slice(line)
            .map_err(|e| CompatError::Failed(format!("unexpected output from borg list: {e}")))?;
        let path = v["path"].as_str().unwrap_or("").as_bytes().to_vec();
        let mode_s = v["mode"].as_str().unwrap_or("");
        let hard = mode_s.starts_with('h');
        let kind = match v["type"].as_str().unwrap_or("-") {
            "d" => Kind::Dir,
            "l" => Kind::Symlink,
            "p" => Kind::Fifo,
            "c" => Kind::CharDevice,
            "b" => Kind::BlockDevice,
            "s" => Kind::Socket,
            _ => Kind::File,
        };
        let (mtime, mtime_nsec) = v["mtime"]
            .as_str()
            .and_then(parse_time)
            .map(|t| (t.as_second(), t.subsec_nanosecond().max(0) as u32))
            .unwrap_or((0, 0));
        let source = v["source"].as_str().unwrap_or("").as_bytes().to_vec();
        let size = v["size"].as_u64().unwrap_or(0);
        let inode_key = if hard && !source.is_empty() {
            source.clone()
        } else {
            path.clone()
        };
        if kind == Kind::File && !hard {
            sizes.insert(path.clone(), size);
        }
        out.push(Entry {
            kind,
            mode: parse_mode(mode_s),
            uid: v["uid"].as_u64().unwrap_or(0) as u32,
            gid: v["gid"].as_u64().unwrap_or(0) as u32,
            size,
            mtime,
            mtime_nsec,
            dev: 1,
            ino: u64::from_str_radix(&fnv(&inode_key), 16).unwrap_or(0),
            nlink: 1,
            rdev: 0,
            link: (kind == Kind::Symlink)
                .then(|| v["linktarget"].as_str().unwrap_or("").as_bytes().to_vec()),
            hash: None,
            xattrs: Vec::new(),
            path,
        });
    }
    // Hard-linked paths: the size of the first one, and a link count.
    let mut counts: HashMap<u64, u64> = HashMap::new();
    for e in out.iter().filter(|e| e.kind == Kind::File) {
        *counts.entry(e.ino).or_default() += 1;
    }
    let by_ino: HashMap<u64, u64> = out
        .iter()
        .filter(|e| e.kind == Kind::File && sizes.contains_key(&e.path))
        .map(|e| (e.ino, e.size))
        .collect();
    for e in out.iter_mut().filter(|e| e.kind == Kind::File) {
        e.nlink = counts.get(&e.ino).copied().unwrap_or(1);
        if e.nlink > 1
            && let Some(&s) = by_ino.get(&e.ino)
        {
            e.size = s;
        }
    }
    // Borg stores the backed-up folders but not their parents (`home` for `/home/me`);
    // browsing and mounting walk down from `/`, so add them as plain folders.
    let present: std::collections::HashSet<Vec<u8>> = out.iter().map(|e| e.path.clone()).collect();
    let mut parents: BTreeMap<Vec<u8>, Entry> = BTreeMap::new();
    for e in &out {
        let mut p = e.path.as_slice();
        while let Some(i) = p.iter().rposition(|&b| b == b'/') {
            p = &p[..i];
            if present.contains(p) || parents.contains_key(p) {
                break;
            }
            parents.insert(
                p.to_vec(),
                Entry {
                    path: p.to_vec(),
                    kind: Kind::Dir,
                    mode: 0o755,
                    nlink: 2,
                    size: 0,
                    ino: u64::from_str_radix(&fnv(p), 16).unwrap_or(0),
                    link: None,
                    xattrs: Vec::new(),
                    hash: None,
                    rdev: 0,
                    ..e.clone()
                },
            );
        }
    }
    out.extend(parents.into_values());
    out.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// An archive as a restore source: the requested paths are extracted into a staging
/// folder with `borg extract`, then restored from there by the generic restore (so the
/// same rules apply as everywhere: identical files skipped, differing ones kept aside,
/// symlink-safe). Dry runs only list the archive.
pub struct BorgSource {
    staging: Option<PathBuf>,
    entries: Vec<Entry>,
}

impl BorgSource {
    /// `staging_parent`: a folder on the destination's side (the `--to` folder, or the
    /// cache folder for restores in place).
    pub fn prepare(
        repo: &BorgRepo,
        snap: &SnapshotMeta,
        paths: &[PathBuf],
        staging_parent: &Path,
        dry_run: bool,
        progress: &Progress,
        report: Reporter<'_>,
    ) -> Result<Self, CompatError> {
        if dry_run {
            return Ok(Self {
                staging: None,
                entries: repo.entries(snap)?,
            });
        }
        std::fs::create_dir_all(staging_parent).map_err(|e| CompatError::io(staging_parent, e))?;
        let staging = staging_parent.join(format!(
            ".flux-borg-restore-{}-{}",
            std::process::id(),
            Timestamp::now().as_millisecond()
        ));
        std::fs::create_dir(&staging).map_err(|e| CompatError::io(&staging, e))?;
        let mut this = Self {
            staging: Some(staging.clone()),
            entries: Vec::new(),
        };
        report(Event::Info {
            message: format!("extracting from archive {} with borg", snap.id),
        });
        progress.set_phase(Phase::Transferring);
        for w in repo.extract(&snap.id, paths, &staging, progress)? {
            if !w.contains("never matched") {
                report(Event::Warning {
                    path: String::new(),
                    message: w,
                });
            }
        }
        progress.bytes_done.store(0, Relaxed);
        this.entries = scan_local(
            &staging,
            XattrPolicy {
                acl: true,
                other: true,
            },
        )
        .map_err(|e| CompatError::io(&staging, e))?;
        Ok(this)
    }
}

impl Drop for BorgSource {
    fn drop(&mut self) {
        if let Some(s) = &self.staging {
            let _ = flux_fs::fsops::make_tree_writable(s);
            let _ = std::fs::remove_dir_all(s);
        }
    }
}

impl flux_fs::restore::RestoreSource for BorgSource {
    fn entries(&self) -> std::io::Result<Box<dyn Iterator<Item = std::io::Result<Entry>> + '_>> {
        Ok(Box::new(self.entries.iter().cloned().map(Ok)))
    }

    fn write_file(
        &self,
        e: &Entry,
        out: &std::fs::File,
        copier: &Copier,
        progress: &Progress,
    ) -> std::io::Result<u64> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| std::io::Error::other("nothing was extracted"))?;
        copier.copy_to(&staging.join(e.rel_path()), out, progress)
    }
}

/// The patterns file (`borg create --patterns-from`): the profile's gitignore-style
/// exclusions, reversed for Borg's first-match-wins order. `!` excludes without looking
/// inside, which is what gitignore does with excluded folders.
pub fn patterns(profile: &Profile, heading: &str, never: &[PathBuf]) -> String {
    let home = sys::home_dir();
    let mut s = format!(
        "# {heading}\n\
         # Borg patterns (borg create --patterns-from). The first pattern that matches a path\n\
         # decides: \"! \" leaves it out (and doesn't look inside), \"+ \" keeps it.\n\
         # Paths have no leading slash. \"P sh\": shell-style, ** matches across folders.\n\
         P sh\n"
    );
    if !never.is_empty() {
        s.push_str("\n# The backup folders themselves:\n");
        for n in never {
            if let Some(p) = n.to_str().filter(|p| !p.contains('\n')) {
                s.push_str(&format!("! pp:{}\n", p.trim_start_matches('/')));
            }
        }
    }
    let rules: Vec<String> = profile
        .sources
        .exclude
        .iter()
        .rev()
        .filter_map(|p| translate(p, home.as_deref()))
        .collect();
    if !rules.is_empty() {
        s.push_str("\n# Exclusions from the profile:\n");
        for r in rules {
            s.push_str(&r);
            s.push('\n');
        }
    }
    s
}

/// One gitignore-style pattern as a Borg shell-style pattern line.
pub fn translate(pattern: &str, home: Option<&Path>) -> Option<String> {
    let pattern = pattern.trim();
    let (sign, body) = match pattern.strip_prefix('!') {
        Some(rest) => ('+', rest),
        None => ('!', pattern),
    };
    let body = match (body.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", h.display().to_string().trim_end_matches('/'))
        }
        _ => body.to_owned(),
    };
    // Borg patterns can't tell folders from files; a trailing slash just goes.
    let body = body.trim_end_matches('/');
    if body.is_empty() || body.starts_with('#') {
        return None;
    }
    let pat = if body.starts_with("**/") {
        body.to_owned()
    } else if let Some(abs) = body.strip_prefix('/') {
        abs.to_owned()
    } else if body.contains('/') {
        body.to_owned()
    } else {
        format!("**/{body}")
    };
    Some(format!("{sign} {pat}"))
}

/// Folders that are never backed up: the repositories on a local target.
pub fn never_paths(profile: &Profile) -> Vec<PathBuf> {
    match &profile.target {
        Target::Local { path, .. } => vec![path.join(ROOT_DIR)],
        Target::Ssh { .. } => Vec::new(),
    }
}

/// `borg create` options from the profile (without the repository and sources).
pub fn create_options(p: &Profile, opts: &BorgOptions, patterns_file: &str) -> Vec<String> {
    let mut a = vec![format!("--compression={}", opts.compression)];
    if p.sources.exclude_caches {
        a.push("--exclude-caches".into());
    }
    for m in &p.sources.exclude_if_present {
        a.push(format!("--exclude-if-present={m}"));
    }
    if p.sources.one_file_system {
        a.push("--one-file-system".into());
    }
    if !p.options.preserve_acl {
        a.push("--noacls".into());
    }
    if !p.options.preserve_xattr {
        a.push("--noxattrs".into());
    }
    if p.options.change_detection == ChangeDetection::Checksum {
        a.push("--files-cache=disabled".into());
    }
    a.push(format!("--patterns-from={patterns_file}"));
    a.extend(opts.extra_args.iter().cloned());
    a
}

pub struct BackupRequest<'a> {
    pub paths: &'a Paths,
    pub id: &'a ProfileId,
    pub profile: &'a Profile,
    pub name: Option<String>,
    pub dry_run: bool,
    pub prune: bool,
    pub batch: bool,
    /// For encrypted repositories (also used to create one).
    pub passphrase: Option<Zeroizing<String>>,
}

/// Does the profile's repository exist yet? (To decide whether to ask for a new
/// passphrase.)
pub fn repository_exists(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    batch: bool,
) -> Result<bool, CompatError> {
    Ok(BorgRepo::for_profile(paths, id, profile, None, batch)?.exists())
}

/// Take a snapshot (a Borg archive).
pub fn backup(
    req: &BackupRequest<'_>,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<BackupOutcome, CompatError> {
    let started = Timestamp::now();
    let p = req.profile;
    let opts = p.borg.clone().unwrap_or_default();
    progress.set_phase(Phase::Preparing);
    let borg_version = tools::require_borg()?;
    let repo = BorgRepo::for_profile(req.paths, req.id, p, req.passphrase.clone(), req.batch)?;
    if let Some(ssh) = repo.store.ssh() {
        let remote = opts.remote_path.clone().unwrap_or_else(|| "borg".into());
        ssh.run(&format!(
            "command -v {} >/dev/null || {{ echo 'borg is not installed on the server' >&2; exit 1; }}",
            shell_quote(&remote)
        ))
        .map_err(CompatError::remote)?;
    }
    if !repo.exists() {
        if req.dry_run {
            report(Event::Info {
                message: "no Borg repository yet; the first backup creates it".into(),
            });
            return Ok(BackupOutcome::DryRun {
                stats: PlanStats::default(),
                errors: Vec::new(),
            });
        }
        report(Event::Info {
            message: format!(
                "creating the Borg repository {} ({})",
                repo.describe(),
                opts.encryption.as_arg()
            ),
        });
        repo.init(opts.encryption)?;
    }

    // Sources that exist.
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    for s in flux_fs::scan::normalize_sources(&p.sources.paths) {
        if s.symlink_metadata().is_ok() {
            sources.push(s);
        } else {
            report(Event::Warning {
                path: s.display().to_string(),
                message: "source does not exist; skipped".into(),
            });
            progress.warnings.fetch_add(1, Relaxed);
            errors.push(FileError {
                path: s.display().to_string(),
                message: "source does not exist".into(),
            });
        }
    }
    if sources.is_empty() {
        return Err(CompatError::NoSources);
    }

    let snaps = repo.snapshots()?;
    if !req.dry_run {
        repo.adopt(&snaps, report);
    }
    let cached = repo.load_cached();
    // Every run makes an archive, as borg users expect: with deduplication an unchanged
    // one costs next to nothing. (Borg can't tell reliably that nothing changed: its files
    // cache forgets the most recently changed files on purpose, and new chunks don't show
    // renames.)
    let prev_stats = snaps
        .last()
        .and_then(|s| cached.get(&s.id))
        .map(|c| c.stats.clone());

    // The patterns file: kept next to the repository, so the command line borg records in
    // each archive points at it. (SSH targets: a local copy, uploaded for reference.)
    let heading = format!(
        "Flux Backup: exclusion patterns for \u{201c}{}\u{201d} ({})",
        p.name, req.id
    );
    let pattern_text = patterns(p, &heading, &never_paths(p));
    let local_patterns = match repo.store.local_path(PATTERNS_FILE) {
        Some(path) if !req.dry_run => {
            repo.store
                .write(PATTERNS_FILE, pattern_text.as_bytes(), 0o644)?;
            path
        }
        _ => {
            std::fs::create_dir_all(&req.paths.cache_dir)
                .map_err(|e| CompatError::io(&req.paths.cache_dir, e))?;
            let path = req
                .paths
                .cache_dir
                .join(format!(".flux-borg-patterns-{}", std::process::id()));
            std::fs::write(&path, &pattern_text).map_err(|e| CompatError::io(&path, e))?;
            if !req.dry_run {
                let _ = repo
                    .store
                    .write(PATTERNS_FILE, pattern_text.as_bytes(), 0o644);
            }
            path
        }
    };

    let names: Vec<String> = snaps.iter().map(|s| s.id.clone()).collect();
    let id = crate::new_snapshot_id(started, |c| names.iter().any(|n| n == c));
    let comment = FluxComment {
        flux: 1,
        name: req.name.clone(),
        pinned: false,
    };
    let mut args: Vec<String> = vec![
        "create".into(),
        "--json".into(),
        "--progress".into(),
        "--list".into(),
        "--filter=AME".into(),
        format!("--comment={}", comment.to_arg()),
    ];
    if req.dry_run {
        args.push("--dry-run".into());
        args.retain(|a| a != "--json");
    }
    args.extend(create_options(
        p,
        &opts,
        &local_patterns.display().to_string(),
    ));
    args.push(repo.arg(Some(&id)));
    args.extend(sources.iter().map(|s| s.display().to_string()));

    if let Some(st) = &prev_stats {
        progress.bytes_total.store(st.total_bytes, Relaxed);
        progress.entries_total.store(st.files, Relaxed);
    }
    progress.set_phase(Phase::Transferring);
    report(Event::Info {
        message: format!(
            "running {}",
            crate::rsync::shell_line(&tools::borg_program(), &args)
        ),
    });
    let (mut added, mut modified) = (0u64, 0u64);
    let mut file_errors = Vec::new();
    let run = repo.run(
        &args,
        None,
        Some(progress),
        &mut |v| match v["type"].as_str() {
            Some("archive_progress") => {
                if let Some(n) = v["original_size"].as_u64() {
                    progress.bytes_done.store(n, Relaxed);
                    let total = progress.bytes_total.load(Relaxed);
                    if n > total {
                        progress.bytes_total.store(n, Relaxed);
                    }
                }
                if let Some(n) = v["nfiles"].as_u64() {
                    progress.entries_done.store(n, Relaxed);
                }
            }
            Some("file_status") => match v["status"].as_str() {
                Some("A") => added += 1,
                Some("M") => modified += 1,
                _ => {}
            },
            Some("log_message") if v["levelname"] == "WARNING" => {
                let msg = v["message"].as_str().unwrap_or("");
                let (path, message) = match msg.split_once(": ") {
                    Some((p, m)) if p.starts_with('/') => (p.to_owned(), m.to_owned()),
                    _ => (String::new(), msg.to_owned()),
                };
                if v["msgid"] == "BackupFileNotFoundError" && errors_has(&errors, &path) {
                    return;
                }
                report(Event::Warning {
                    path: path.clone(),
                    message: message.clone(),
                });
                progress.warnings.fetch_add(1, Relaxed);
                file_errors.push(FileError { path, message });
            }
            _ => {}
        },
    );
    let _ = std::fs::remove_file(
        req.paths
            .cache_dir
            .join(format!(".flux-borg-patterns-{}", std::process::id())),
    );
    let run = run?.ok("create")?;
    errors.extend(file_errors);

    if req.dry_run {
        return Ok(BackupOutcome::DryRun {
            stats: PlanStats {
                new: added,
                changed: modified,
                copy_files: added + modified,
                ..Default::default()
            },
            errors,
        });
    }

    let v: Value = serde_json::from_slice(&run.stdout).unwrap_or(Value::Null);
    let st = &v["archive"]["stats"];
    let files = st["nfiles"].as_u64().unwrap_or(0);
    let finished = Timestamp::now();
    progress.set_phase(Phase::Finalizing);

    if !p.options.continue_on_errors && !errors.is_empty() {
        let _ = repo.delete(&[id.as_str()], report);
        return Err(CompatError::Failed(format!(
            "{} items could not be backed up and continue_on_errors is off; the archive was removed",
            errors.len()
        )));
    }
    let errors_total = errors.len() as u64;
    let mut stored_errors = errors;
    stored_errors.truncate(MAX_STORED_ERRORS);
    let total_bytes = st["original_size"].as_u64().unwrap_or(0);
    let stats = SnapshotStats {
        entries: files,
        files,
        total_bytes,
        new: added,
        changed: modified,
        unchanged: files.saturating_sub(added + modified),
        // Borg doesn't report the bytes it read; what it stored is the useful number.
        copied_bytes: st["deduplicated_size"].as_u64().unwrap_or(0),
        copied_files: added + modified,
        linked_files: files.saturating_sub(added + modified),
        stored_bytes: st["deduplicated_size"].as_u64().unwrap_or(0),
        ..Default::default()
    };
    let status = if errors_total == 0 {
        SnapshotStatus::Complete
    } else {
        SnapshotStatus::CompleteWithErrors
    };
    repo.remember(
        &id,
        Cached {
            stats: stats.clone(),
            status: Some(status),
            errors: stored_errors.iter().take(200).cloned().collect(),
            errors_total,
            sources: p.sources.paths.clone(),
        },
    );
    let meta = SnapshotMeta {
        format: 1,
        id: id.clone(),
        profile: req.id.to_string(),
        host: sys::hostname(),
        user: sys::username(),
        started,
        finished,
        status,
        name: req.name.clone(),
        pinned: false,
        sources: p.sources.paths.clone(),
        stats,
        errors: stored_errors,
        errors_total,
        flux_version: flux_core::VERSION.into(),
    };
    let pruned = if req.prune {
        prune_inner(&repo, &p.retention, false, report)?.removed
    } else {
        Vec::new()
    };
    write_kit(&repo, req, &borg_version, &id, report);
    progress.set_phase(Phase::Done);
    Ok(BackupOutcome::Created {
        snapshot: Box::new(meta),
        pruned,
    })
}

fn errors_has(errors: &[FileError], path: &str) -> bool {
    errors.iter().any(|e| e.path == path)
}

fn write_kit(
    repo: &BorgRepo,
    req: &BackupRequest<'_>,
    borg_version: &str,
    newest: &str,
    report: Reporter<'_>,
) {
    let kit = kit::borg_kit(req.id, req.profile, borg_version, Some(newest));
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

fn prune_inner(
    repo: &BorgRepo,
    policy: &flux_core::profile::Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<prune::PruneReport, CompatError> {
    let snaps = repo.snapshots()?;
    let mut doomed: Vec<String> = Vec::new();
    let mut r = prune::apply(
        &snaps,
        policy,
        dry_run,
        &|| repo.store.free_space(),
        &mut |id| {
            repo.run_simple(
                &[
                    "delete".into(),
                    "--lock-wait".into(),
                    LOCK_WAIT.into(),
                    repo.arg(Some(id)),
                ],
                "delete",
            )?;
            repo.cache.forget(id);
            doomed.push(id.to_owned());
            Ok(())
        },
        report,
    )?;
    if !doomed.is_empty() {
        repo.compact()?;
        let mut m = repo.load_cached();
        for d in &doomed {
            m.remove(d);
        }
        repo.save_cached(&m);
    }
    r.dry_run = dry_run;
    Ok(r)
}

/// `flux prune` for a Borg repository: Flux's retention policy (pins and names
/// included), applied with `borg delete`, then `borg compact`.
pub fn prune_repo(
    repo: &BorgRepo,
    policy: &flux_core::profile::Retention,
    dry_run: bool,
    report: Reporter<'_>,
) -> Result<prune::PruneReport, CompatError> {
    prune_inner(repo, policy, dry_run, report)
}

/// `borg check` (with `--verify-data` to read every chunk back).
pub fn check(
    repo: &BorgRepo,
    read_data: bool,
    progress: &Progress,
) -> Result<CheckSummary, CompatError> {
    progress.set_phase(Phase::Scanning);
    let snaps = repo.snapshots()?;
    let mut args = vec!["check".into(), "--progress".into()];
    if read_data {
        args.push("--verify-data".into());
    }
    args.push(repo.arg(None));
    let run = repo.run(&args, None, Some(progress), &mut |v| {
        if v["type"] == "progress_percent"
            && let (Some(c), Some(t)) = (v["current"].as_u64(), v["total"].as_u64())
        {
            progress.entries_total.store(t, Relaxed);
            progress.entries_done.store(c, Relaxed);
        }
    })?;
    let mut sum = CheckSummary {
        snapshots: snaps.len() as u64,
        checked: snaps.len() as u64,
        ..Default::default()
    };
    if read_data
        && let Ok(r) = repo.run_simple(&["info".into(), "--json".into(), repo.arg(None)], "info")
        && let Ok(v) = serde_json::from_slice::<Value>(&r.stdout)
    {
        // --verify-data reads every chunk: the repository's stored size.
        sum.bytes_read = v["cache"]["stats"]["unique_csize"].as_u64().unwrap_or(0);
    }
    for m in &run.messages {
        match m.level.as_str() {
            "ERROR" | "CRITICAL" => sum.errors.push(m.message.clone()),
            "WARNING" => sum.warnings.push(m.message.clone()),
            _ => {}
        }
    }
    match run.code {
        Some(0) => {}
        Some(1) if sum.errors.is_empty() => {}
        _ => {
            if sum.errors.is_empty() {
                // Still map passphrase and lock problems to their own errors.
                run.ok("check")?;
                sum.errors.push("borg check found problems".into());
            }
        }
    }
    progress.set_phase(Phase::Done);
    Ok(sum)
}

/// Archive listings this process got from borg, per repository, with the transaction they
/// were taken at and a keyed hash of the passphrase that was used (under a key that exists
/// only in this process). See [`BorgRepo::snapshots`].
mod listed {
    use std::sync::{LazyLock, Mutex};

    struct Listed {
        url: String,
        marker: String,
        tag: [u8; 32],
        raw: Vec<u8>,
    }

    const KEEP: usize = 16;
    static TAG_KEY: LazyLock<[u8; 32]> = LazyLock::new(|| {
        let mut k = [0u8; 32];
        if getrandom::fill(&mut k).is_err() {
            k = *blake3::hash(format!("{:?}", std::time::Instant::now()).as_bytes()).as_bytes();
        }
        k
    });
    static CACHE: Mutex<Vec<Listed>> = Mutex::new(Vec::new());

    pub(super) fn tag(passphrase: Option<&str>) -> [u8; 32] {
        *blake3::keyed_hash(&TAG_KEY, passphrase.unwrap_or("").as_bytes()).as_bytes()
    }

    pub(super) fn get(url: &str, marker: &str, tag: &[u8; 32]) -> Option<Vec<u8>> {
        let c = CACHE.lock().ok()?;
        c.iter()
            .find(|l| l.url == url && l.marker == marker && l.tag == *tag)
            .map(|l| l.raw.clone())
    }

    pub(super) fn put(url: &str, marker: &str, tag: &[u8; 32], raw: &[u8]) {
        let Ok(mut c) = CACHE.lock() else { return };
        c.retain(|l| l.url != url);
        if c.len() >= KEEP {
            c.remove(0);
        }
        c.push(Listed {
            url: url.to_owned(),
            marker: marker.to_owned(),
            tag: *tag,
            raw: raw.to_vec(),
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_the_same_passphrase_and_transaction_hit() {
            let right = tag(Some("right"));
            put("repo-a", "7 t1", &right, b"listing");
            assert_eq!(
                get("repo-a", "7 t1", &right).as_deref(),
                Some(&b"listing"[..])
            );
            assert!(
                get("repo-a", "7 t1", &tag(Some("wrong"))).is_none(),
                "wrong passphrase"
            );
            assert!(get("repo-a", "7 t1", &tag(None)).is_none(), "no passphrase");
            assert!(get("repo-a", "8 t2", &right).is_none(), "newer transaction");
            assert!(get("repo-b", "7 t1", &right).is_none(), "other repository");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_times() {
        assert_eq!(parse_mode("drwxr-xr-x"), 0o755);
        assert_eq!(parse_mode("-rwsr-sr-t"), 0o7755);
        assert_eq!(parse_mode("hrw-r--r--"), 0o644);
        assert_eq!(parse_mode("-rwSr--r-T"), 0o5644);
        let t = parse_time("2026-11-07T22:20:44.469570").unwrap();
        assert_eq!(t.subsec_nanosecond(), 469_570_000);
        assert_eq!(
            parse_freed("compaction freed about 9.94 kB repository space."),
            Some(9940)
        );
    }

    #[test]
    fn comments_round_trip_and_escape() {
        let c = FluxComment {
            flux: 1,
            name: Some("before {upgrade}".into()),
            pinned: true,
        };
        let arg = c.to_arg();
        assert!(arg.starts_with("{{") && arg.contains("{{upgrade}}"));
        let back = arg.replace("{{", "{").replace("}}", "}");
        assert_eq!(FluxComment::parse(&back), c);
        assert_eq!(FluxComment::parse("made by hand"), FluxComment::default());
    }

    #[test]
    fn translates_patterns() {
        let h = Some(Path::new("/home/me"));
        assert_eq!(translate("*.tmp", h).unwrap(), "! **/*.tmp");
        assert_eq!(translate("node_modules/", h).unwrap(), "! **/node_modules");
        assert_eq!(translate("/home/me/x", h).unwrap(), "! home/me/x");
        assert_eq!(translate("~/Videos", h).unwrap(), "! home/me/Videos");
        assert_eq!(translate("a/b", h).unwrap(), "! a/b");
        assert_eq!(translate("**/.git", h).unwrap(), "! **/.git");
        assert_eq!(translate("!keep.tmp", h).unwrap(), "+ **/keep.tmp");
        assert!(translate("# c", h).is_none());
    }

    #[test]
    fn parses_listings_with_hard_links() {
        let raw = br#"{"type": "d", "mode": "drwxr-xr-x", "uid": 1000, "gid": 1000, "path": "home/me", "source": "", "linktarget": "", "size": 0, "mtime": "2026-11-07T22:20:44.468394"}
{"type": "-", "mode": "-rw-r--r--", "uid": 1000, "gid": 1000, "path": "home/me/b.txt", "source": "", "linktarget": "", "size": 3, "mtime": "2026-11-07T22:20:44.469570"}
{"type": "-", "mode": "hrw-r--r--", "uid": 1000, "gid": 1000, "path": "home/me/a.txt", "source": "home/me/b.txt", "linktarget": "home/me/b.txt", "size": 0, "mtime": "2026-11-07T22:20:44.469570"}
{"type": "l", "mode": "lrwxrwxrwx", "uid": 1000, "gid": 1000, "path": "home/me/l", "source": "a.txt", "linktarget": "a.txt", "size": 0, "mtime": "2026-11-07T22:20:44.469570"}
"#;
        let v = parse_listing(raw).unwrap();
        assert_eq!(v.len(), 5);
        // The parent folder borg doesn't store.
        assert_eq!((v[0].path.as_slice(), v[0].kind), (&b"home"[..], Kind::Dir));
        assert_eq!(v[2].path, b"home/me/a.txt");
        assert_eq!(v[2].ino, v[3].ino);
        assert_eq!((v[2].size, v[2].nlink), (3, 2));
        assert_eq!(v[4].link.as_deref(), Some(&b"a.txt"[..]));
    }

    #[test]
    fn urls() {
        let mut p = Profile::new(
            "x",
            Target::Ssh {
                host: "nas".into(),
                user: Some("me".into()),
                port: Some(2222),
                path: "backups".into(),
                identity_file: None,
                ssh_options: vec![],
                bandwidth_limit: None,
            },
            vec!["/home/me".into()],
        );
        p.engine = flux_core::Engine::Borg;
        let id = ProfileId::new("x").unwrap();
        let url = repo_url(&id, &p, true);
        assert!(
            url.starts_with("ssh://me@nas:2222/./backups/flux-borg/"),
            "{url}"
        );
        assert!(url.ends_with("/x/repo"));
        assert!(!repo_url(&id, &p, false).contains("2222"));
    }
}
