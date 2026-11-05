//! Backup profiles: what to back up, where to, how, and when.
//!
//! A profile is stored as `profiles/<id>.toml`. Unknown keys are rejected: a mistyped
//! `exclude` key that was silently ignored could put files in a backup the user meant to
//! leave out, or skip files they meant to keep.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::units::{ByteSize, HumanDuration};

/// Current on-disk profile schema version.
pub const PROFILE_VERSION: u32 = 1;

/// A validated profile identifier: 1–64 chars of `[a-z0-9_-]`, starting with a letter or
/// digit. Used as the file name and in systemd unit names, hence the strict alphabet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid profile id {0:?}: use 1-64 lowercase letters, digits, '-' or '_', \
     starting with a letter or digit"
)]
pub struct InvalidProfileId(pub String);

impl ProfileId {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidProfileId> {
        let s = s.into();
        let ok = (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            && s.as_bytes()[0].is_ascii_alphanumeric();
        if ok {
            Ok(ProfileId(s))
        } else {
            Err(InvalidProfileId(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Derive an id from a free-form display name ("My Laptop Home!" → "my-laptop-home").
    pub fn slugify(name: &str) -> Option<Self> {
        let mut out = String::new();
        for c in name.chars().flat_map(char::to_lowercase) {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        }
        let out = out.trim_end_matches('-');
        let out = &out[..out.len().min(64)];
        ProfileId::new(out.trim_end_matches('-')).ok()
    }
}

impl TryFrom<String> for ProfileId {
    type Error = InvalidProfileId;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        ProfileId::new(s)
    }
}

impl From<ProfileId> for String {
    fn from(id: ProfileId) -> String {
        id.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Storage engine of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// Hard-link snapshots: every backup is a plain, browsable directory tree.
    #[default]
    Mirror,
    /// Deduplicating, compressed, optionally encrypted chunk repository.
    Vault,
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Engine::Mirror => "mirror",
            Engine::Vault => "vault",
        })
    }
}

/// A complete backup profile as stored on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Schema version, for future migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Human-readable name shown in the GUI.
    pub name: String,
    #[serde(default)]
    pub engine: Engine,
    pub target: Target,
    pub sources: Sources,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub options: Options,
    #[serde(default, skip_serializing_if = "Hooks::is_empty")]
    pub hooks: Hooks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultOptions>,
}

fn default_version() -> u32 {
    PROFILE_VERSION
}

/// Where backups are written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Target {
    /// A locally mounted directory (internal disk, USB drive, mounted network share).
    Local {
        path: PathBuf,
        /// Filesystem UUID of the drive expected at `path`. When set, Flux refuses to write
        /// to `path` unless that filesystem is mounted there, which prevents filling the
        /// root filesystem when a USB drive is unplugged. It also enables backup on connect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fs_uuid: Option<String>,
    },
    /// A remote host reached with the system `ssh` client (honours `~/.ssh/config`).
    Ssh {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        path: PathBuf,
        /// Private key file. When unset, ssh's own config and agent decide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_file: Option<PathBuf>,
        /// Extra `-o Key=Value` options passed to ssh.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ssh_options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bandwidth_limit: Option<ByteSize>,
    },
}

/// What gets backed up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sources {
    /// Absolute paths of files or directories to include.
    pub paths: Vec<PathBuf>,
    /// Exclude patterns (gitignore-style globs: `*.tmp`, `.cache/`, `/home/me/Downloads`).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Skip regular files larger than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_larger_than: Option<ByteSize>,
    /// Skip directories containing a valid `CACHEDIR.TAG`.
    #[serde(default = "yes")]
    pub exclude_caches: bool,
    /// Skip directories that contain a file with one of these names (e.g. `.nobackup`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_if_present: Vec<String>,
    /// Do not cross filesystem boundaries below each source path.
    #[serde(default)]
    pub one_file_system: bool,
}

/// Which snapshots survive pruning. A snapshot is kept if *any* rule keeps it.
///
/// Bucket rules (`hourly` … `yearly`) keep the newest snapshot of each of the last N
/// periods that contain a snapshot, like restic and borg do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    /// Master switch. When false, nothing is ever pruned automatically.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Always keep the N most recent snapshots.
    #[serde(default)]
    pub keep_last: u32,
    /// Keep every snapshot younger than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_within: Option<HumanDuration>,
    #[serde(default)]
    pub keep_hourly: u32,
    #[serde(default)]
    pub keep_daily: u32,
    #[serde(default)]
    pub keep_weekly: u32,
    #[serde(default)]
    pub keep_monthly: u32,
    #[serde(default)]
    pub keep_yearly: u32,
    /// Snapshots older than this are removed even if a bucket rule would keep them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<HumanDuration>,
    /// Never prune snapshots that the user named or pinned.
    #[serde(default = "yes")]
    pub keep_named: bool,
    /// Remove oldest prunable snapshots until at least this much space is free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_space: Option<ByteSize>,
    /// Same, for free inodes, as a percentage (0–100) of the target filesystem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_inodes_percent: Option<u8>,
}

impl Default for Retention {
    /// Sensible defaults: everything from the last day, then 7 daily, 4 weekly, 12 monthly
    /// and 5 yearly snapshots.
    fn default() -> Self {
        Self {
            enabled: true,
            keep_last: 1,
            keep_within: Some(HumanDuration::from_secs(24 * 3600)),
            keep_hourly: 0,
            keep_daily: 7,
            keep_weekly: 4,
            keep_monthly: 12,
            keep_yearly: 5,
            max_age: None,
            keep_named: true,
            min_free_space: None,
            min_free_inodes_percent: None,
        }
    }
}

/// When backups run automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(default)]
    pub trigger: Trigger,
    /// systemd calendar expression for `trigger = "calendar"`, e.g. `daily`,
    /// `Mon..Fri 18:00`, `*-*-* 03:30`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_calendar: Option<String>,
    /// Interval for `trigger = "interval"`, counted from the last successful backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<HumanDuration>,
    /// Run a missed backup at the next opportunity (e.g. machine was off at the time).
    #[serde(default = "yes")]
    pub catch_up: bool,
    /// Do not start scheduled backups while running on battery.
    #[serde(default)]
    pub skip_on_battery: bool,
    /// Also back up when the target drive (`target.fs_uuid`) is connected.
    #[serde(default)]
    pub on_connect: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            trigger: Trigger::Manual,
            on_calendar: None,
            every: None,
            catch_up: true,
            skip_on_battery: false,
            on_connect: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// Only when started by hand (or by `on_connect`).
    #[default]
    Manual,
    /// At times given by `on_calendar`.
    Calendar,
    /// Every `every`.
    Interval,
}

/// How symbolic links are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkMode {
    /// Store links as links.
    #[default]
    Preserve,
    /// Copy what links point to when the target lies outside the source tree.
    FollowUnsafe,
    /// Always copy what links point to.
    Follow,
}

/// How unchanged files are detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDetection {
    /// Size, mtime, ctime, inode, mode (fast).
    #[default]
    Metadata,
    /// Also hash file contents (slow; catches silent corruption and mtime-preserving edits).
    Checksum,
}

/// I/O scheduling class for backup processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IoPriority {
    Normal,
    #[default]
    BestEffortLow,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default = "yes")]
    pub preserve_acl: bool,
    #[serde(default = "yes")]
    pub preserve_xattr: bool,
    #[serde(default)]
    pub symlinks: SymlinkMode,
    #[serde(default)]
    pub change_detection: ChangeDetection,
    /// Finish the snapshot even if some files could not be read (they're listed in the log).
    #[serde(default = "yes")]
    pub continue_on_errors: bool,
    /// Create a new snapshot even if nothing changed since the last one.
    #[serde(default)]
    pub snapshot_if_unchanged: bool,
    /// CPU niceness for backup processes (0–19).
    #[serde(default = "default_nice")]
    pub nice: u8,
    #[serde(default)]
    pub io_priority: IoPriority,
    /// Send desktop notifications on completion and failure.
    #[serde(default = "yes")]
    pub notify: bool,
    /// When restoring over existing files, first move them aside (`name.flux-<timestamp>`).
    #[serde(default = "yes")]
    pub backup_before_restore: bool,
    /// Worker threads for scanning/copying. 0 = automatic.
    #[serde(default)]
    pub threads: u16,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            preserve_acl: true,
            preserve_xattr: true,
            symlinks: SymlinkMode::default(),
            change_detection: ChangeDetection::default(),
            continue_on_errors: true,
            snapshot_if_unchanged: false,
            nice: default_nice(),
            io_priority: IoPriority::default(),
            notify: true,
            backup_before_restore: true,
            threads: 0,
        }
    }
}

fn default_nice() -> u8 {
    10
}

/// Shell commands run around a backup. Each runs with `sh -c` and receives `FLUX_*`
/// environment variables describing the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// Runs before the backup; a non-zero exit aborts the backup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_backup: Vec<String>,
    /// Runs after a successful backup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_backup: Vec<String>,
    /// Runs when a backup fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_error: Vec<String>,
    /// Kill a hook that runs longer than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<HumanDuration>,
}

impl Hooks {
    pub fn is_empty(&self) -> bool {
        self.pre_backup.is_empty()
            && self.post_backup.is_empty()
            && self.on_error.is_empty()
            && self.timeout.is_none()
    }
}

/// Vault-engine settings. Only meaningful when `engine = "vault"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultOptions {
    /// zstd level, 0 = no compression, 1–22.
    #[serde(default = "default_zstd")]
    pub compression_level: u8,
    /// Encrypt repository contents. Fixed when the repository is created.
    #[serde(default = "yes")]
    pub encryption: bool,
    /// Where the repository password comes from.
    #[serde(default)]
    pub password_source: PasswordSource,
}

impl Default for VaultOptions {
    fn default() -> Self {
        Self {
            compression_level: default_zstd(),
            encryption: true,
            password_source: PasswordSource::default(),
        }
    }
}

fn default_zstd() -> u8 {
    3
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum PasswordSource {
    /// Desktop keyring via the Secret Service API.
    #[default]
    Keyring,
    /// Ask interactively every time.
    Prompt,
    /// Read the first line of a file (should be mode 0600).
    File { path: PathBuf },
    /// Run a command and use its first output line (e.g. `pass show backup`).
    Command { command: String },
}

fn yes() -> bool {
    true
}

impl Profile {
    /// A new profile with default settings.
    pub fn new(name: impl Into<String>, target: Target, sources: Vec<PathBuf>) -> Self {
        Self {
            version: PROFILE_VERSION,
            name: name.into(),
            engine: Engine::Mirror,
            target,
            sources: Sources {
                paths: sources,
                exclude: Vec::new(),
                exclude_larger_than: None,
                exclude_caches: true,
                exclude_if_present: Vec::new(),
                one_file_system: false,
            },
            retention: Retention::default(),
            schedule: Schedule::default(),
            options: Options::default(),
            hooks: Hooks::default(),
            vault: None,
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Check the profile for problems. Errors make the profile unusable; warnings do not.
    /// [`Profile::validate`] plus what only matters in `scope`: system backups run without
    /// anyone logged in, so their Vault password must come from a file or a command.
    pub fn validate_in(&self, scope: crate::Scope) -> Vec<Issue> {
        let mut issues = self.validate();
        if scope == crate::Scope::System
            && self.engine == Engine::Vault
            && self.vault.as_ref().is_none_or(|v| v.encryption)
            && matches!(
                self.vault.as_ref().map(|v| &v.password_source),
                None | Some(PasswordSource::Keyring | PasswordSource::Prompt)
            )
        {
            issues.push(Issue {
                severity: Severity::Warning,
                field: "vault.password_source".into(),
                message: "system backups run without anyone logged in, so they can't use a keyring or ask for the password; use a key file readable by root only (password_source = { file = \"/etc/flux-backup/keys/<id>\" })".into(),
            });
        }
        issues
    }

    pub fn validate(&self) -> Vec<Issue> {
        let mut v = Vec::new();
        let mut err = |field: &str, msg: String| v.push(Issue::error(field, msg));

        if self.version > PROFILE_VERSION {
            err(
                "version",
                format!(
                    "profile was written by a newer Flux (schema {}, this build supports {})",
                    self.version, PROFILE_VERSION
                ),
            );
        }
        if self.name.trim().is_empty() {
            err("name", "name must not be empty".into());
        }

        match &self.target {
            Target::Local { path, fs_uuid } => {
                if !path.is_absolute() {
                    err(
                        "target.path",
                        format!("{} is not an absolute path", path.display()),
                    );
                }
                if fs_uuid.as_deref().is_some_and(|u| u.trim().is_empty()) {
                    err(
                        "target.fs_uuid",
                        "fs_uuid must not be empty when set".into(),
                    );
                }
            }
            Target::Ssh {
                host,
                port,
                path,
                user,
                ..
            } => {
                if host.trim().is_empty() || host.contains(char::is_whitespace) {
                    err(
                        "target.host",
                        "host must be a non-empty name without spaces".into(),
                    );
                }
                if user
                    .as_deref()
                    .is_some_and(|u| u.is_empty() || u.contains(['@', ' ']))
                {
                    err("target.user", "invalid user name".into());
                }
                if *port == Some(0) {
                    err("target.port", "port must be 1-65535".into());
                }
                if path.as_os_str().is_empty() {
                    err("target.path", "remote path must not be empty".into());
                }
            }
        }

        if self.sources.paths.is_empty() {
            err(
                "sources.paths",
                "add at least one file or folder to back up".into(),
            );
        }
        for (i, p) in self.sources.paths.iter().enumerate() {
            if !p.is_absolute() {
                err(
                    &format!("sources.paths[{i}]"),
                    format!("{} is not absolute", p.display()),
                );
            }
        }
        for (i, pat) in self.sources.exclude.iter().enumerate() {
            if pat.trim().is_empty() {
                err(
                    &format!("sources.exclude[{i}]"),
                    "empty exclude pattern".into(),
                );
            }
        }
        if let Target::Local { path, .. } = &self.target {
            for (i, src) in self.sources.paths.iter().enumerate() {
                if path.starts_with(src) && !self.excludes_path(path) {
                    v.push(Issue::warning(
                        &format!("sources.paths[{i}]"),
                        format!(
                            "the backup target {} is inside this source; Flux will skip it",
                            path.display()
                        ),
                    ));
                }
            }
        }

        let r = &self.retention;
        if r.enabled
            && r.keep_last == 0
            && r.keep_within.is_none()
            && r.keep_hourly + r.keep_daily + r.keep_weekly + r.keep_monthly + r.keep_yearly == 0
            && !r.keep_named
        {
            v.push(Issue::error(
                "retention",
                "retention is enabled but no rule keeps anything: every snapshot would be pruned"
                    .into(),
            ));
        }
        if r.min_free_inodes_percent.is_some_and(|p| p > 100) {
            v.push(Issue::error(
                "retention.min_free_inodes_percent",
                "must be 0-100".into(),
            ));
        }

        let sch = &self.schedule;
        match sch.trigger {
            Trigger::Calendar
                if sch
                    .on_calendar
                    .as_deref()
                    .is_none_or(|c| c.trim().is_empty()) =>
            {
                v.push(Issue::error(
                    "schedule.on_calendar",
                    "a calendar schedule needs on_calendar (e.g. \"daily\")".into(),
                ));
            }
            Trigger::Interval if sch.every.is_none_or(|e| e.0.as_secs() < 60) => {
                v.push(Issue::error(
                    "schedule.every",
                    "an interval schedule needs every >= 1 minute".into(),
                ));
            }
            _ => {}
        }
        if self.schedule.on_connect
            && !matches!(
                &self.target,
                Target::Local {
                    fs_uuid: Some(_),
                    ..
                }
            )
        {
            v.push(Issue::error(
                "schedule.on_connect",
                "backup on connect needs a local target with target.fs_uuid set".into(),
            ));
        }

        if self.options.nice > 19 {
            v.push(Issue::error("options.nice", "nice must be 0-19".into()));
        }

        match (self.engine, &self.vault) {
            (Engine::Vault, Some(vo)) if vo.compression_level > 22 => {
                v.push(Issue::error(
                    "vault.compression_level",
                    "zstd level must be 0-22".into(),
                ));
            }
            (Engine::Mirror, Some(_)) => v.push(Issue::warning(
                "vault",
                "[vault] settings are ignored for mirror profiles".into(),
            )),
            _ => {}
        }

        v
    }

    /// True if `path` is covered by an exclude pattern that is an absolute path prefix.
    /// (Full glob evaluation lives in the filter engine; this is only for the target check.)
    fn excludes_path(&self, path: &Path) -> bool {
        self.sources.exclude.iter().any(|pat| {
            let p = Path::new(pat.trim_end_matches('/'));
            p.is_absolute() && path.starts_with(p)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A validation finding, addressed by a dotted field path so the GUI can attach it to the
/// right form control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub field: String,
    pub message: String,
}

impl Issue {
    fn error(field: &str, message: String) -> Self {
        Self {
            severity: Severity::Error,
            field: field.into(),
            message,
        }
    }
    fn warning(field: &str, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            field: field.into(),
            message,
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{s}: {}: {}", self.field, self.message)
    }
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        Profile::new(
            "Home",
            Target::Local {
                path: "/mnt/backup".into(),
                fs_uuid: None,
            },
            vec!["/home/me".into()],
        )
    }

    #[test]
    fn ids() {
        assert!(ProfileId::new("home").is_ok());
        assert!(ProfileId::new("my_home-2").is_ok());
        assert!(ProfileId::new("").is_err());
        assert!(ProfileId::new("-home").is_err());
        assert!(ProfileId::new("Home").is_err());
        assert!(ProfileId::new("a/b").is_err());
        assert!(ProfileId::new("x".repeat(65)).is_err());
        assert_eq!(
            ProfileId::slugify("My Laptop  Home!").unwrap().as_str(),
            "my-laptop-home"
        );
        assert!(ProfileId::slugify("!!!").is_none());
    }

    #[test]
    fn default_profile_is_valid_and_round_trips() {
        let p = sample();
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        let s = p.to_toml().unwrap();
        assert_eq!(Profile::from_toml(&s).unwrap(), p);
    }

    #[test]
    fn minimal_toml_gets_defaults() {
        let p = Profile::from_toml(
            r#"
            name = "Docs"
            [target]
            kind = "ssh"
            host = "nas"
            path = "/srv/backup"
            [sources]
            paths = ["/home/me/Documents"]
            exclude = ["*.tmp"]
            exclude_larger_than = "2GiB"
            [schedule]
            trigger = "calendar"
            on_calendar = "daily"
            "#,
        )
        .unwrap();
        assert_eq!(p.engine, Engine::Mirror);
        assert_eq!(p.sources.exclude_larger_than, Some(ByteSize(2 << 30)));
        assert!(p.sources.exclude_caches);
        assert_eq!(p.retention, Retention::default());
        assert_eq!(p.schedule.trigger, Trigger::Calendar);
        assert!(p.schedule.catch_up);
        assert!(!has_errors(&p.validate()));
    }

    #[test]
    fn rejects_unknown_keys() {
        let e = Profile::from_toml(
            r#"
            name = "x"
            [target]
            kind = "local"
            path = "/mnt"
            [sources]
            paths = ["/home"]
            exclud = ["typo"]
            "#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("exclud"), "{e}");
    }

    #[test]
    fn validation_catches_problems() {
        let mut p = sample();
        p.name = " ".into();
        p.sources.paths = vec!["relative/path".into()];
        p.schedule.on_connect = true;
        p.retention = Retention {
            enabled: true,
            keep_last: 0,
            keep_within: None,
            keep_hourly: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: 0,
            max_age: None,
            keep_named: false,
            min_free_space: None,
            min_free_inodes_percent: None,
        };
        let fields: Vec<_> = p.validate().into_iter().map(|i| i.field).collect();
        for f in [
            "name",
            "sources.paths[0]",
            "schedule.on_connect",
            "retention",
        ] {
            assert!(fields.iter().any(|x| x == f), "missing {f} in {fields:?}");
        }
    }

    #[test]
    fn warns_when_target_inside_source() {
        let mut p = sample();
        p.target = Target::Local {
            path: "/home/me/backups".into(),
            fs_uuid: None,
        };
        let issues = p.validate();
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.severity == Severity::Warning));
        p.sources.exclude.push("/home/me/backups/".into());
        assert!(p.validate().is_empty());
    }
}
