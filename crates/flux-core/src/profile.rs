//! Backup profiles: what to back up, where to, how, and when.
//!
//! A profile is stored as `profiles/<id>.toml`. Unknown keys are rejected: a mistyped
//! `exclude` key that was silently ignored could put files in a backup the user meant to
//! leave out, or skip files they meant to keep.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::units::{ByteSize, HumanDuration, Percent};

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

/// Joins a profile's id and one of its extra destinations: `home+nas`.
pub const DESTINATION_SEP: char = '+';

impl ProfileId {
    /// A profile id (`home`), or a destination of one (`home+nas`): lowercase letters,
    /// digits, `-` and `_`, starting with a letter or digit, joined by one `+` for a
    /// destination.
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidProfileId> {
        let s = s.into();
        let part = |p: &str| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
                && p.as_bytes()[0].is_ascii_alphanumeric()
        };
        let ok = (1..=129).contains(&s.len())
            && match s.split_once(DESTINATION_SEP) {
                None => s.len() <= 64 && part(&s),
                Some((base, dest)) => {
                    base.len() <= 64 && dest.len() <= 64 && part(base) && part(dest)
                }
            };
        if ok {
            Ok(ProfileId(s))
        } else {
            Err(InvalidProfileId(s))
        }
    }

    /// The id of destination `dest` of this profile: `home` + `nas` → `home+nas`.
    pub fn with_destination(&self, dest: &str) -> Result<Self, InvalidProfileId> {
        ProfileId::new(format!("{}{DESTINATION_SEP}{dest}", self.0))
    }

    /// For a destination's id: the profile's id and the destination's name.
    pub fn split_destination(&self) -> Option<(ProfileId, &str)> {
        let (base, dest) = self.0.split_once(DESTINATION_SEP)?;
        Some((ProfileId(base.to_owned()), dest))
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
    /// Compatibility mode: plain folders written by the system `rsync` (`--link-dest`
    /// snapshots), usable without Flux.
    Rsync,
    /// Compatibility mode: a standard Borg repository written by the system `borg`.
    Borg,
}

impl Engine {
    /// Compatibility-mode engines store everything in a standard tool's native format.
    pub fn is_compat(self) -> bool {
        matches!(self, Engine::Rsync | Engine::Borg)
    }

    /// Name shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Engine::Mirror => "Mirror",
            Engine::Vault => "Vault",
            Engine::Rsync => "rsync",
            Engine::Borg => "Borg",
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Engine::Mirror => "mirror",
            Engine::Vault => "vault",
            Engine::Rsync => "rsync",
            Engine::Borg => "borg",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsync: Option<RsyncOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borg: Option<BorgOptions>,
    /// Upkeep done after scheduled backups when it's due.
    #[serde(default, skip_serializing_if = "Maintenance::is_default")]
    pub maintenance: Maintenance,
    /// The main destination (`target`) is away from this computer's building: a server or
    /// a drive kept elsewhere. Shown, and used by the "offsite" filter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offsite: bool,
    /// More places the same backup goes, each with its own way of storing it. Every
    /// backup run goes to all of them, one after the other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<Destination>,
    /// Set (never stored) on the profile of a destination that is a copy of the main
    /// one ([`CopyMode::Replicate`]): its snapshots are the main profile's.
    #[serde(skip)]
    pub replica: bool,
    /// Wi-Fi rules and upload limits by time of day, for backups to a server.
    #[serde(default, skip_serializing_if = "crate::network::Network::is_default")]
    pub network: crate::network::Network,
    /// Restore tests: now and then, restore a sample and check it.
    #[serde(default, skip_serializing_if = "RestoreTest::is_default")]
    pub restore_test: RestoreTest,
    /// How readily a backup's changes count as unusual (a sign of ransomware, or of a
    /// mistake): many more files changed than usual, files renamed to a new extension,
    /// text that now looks encrypted, ransom notes.
    #[serde(default, skip_serializing_if = "Sensitivity::is_normal")]
    pub unusual_changes: Sensitivity,
    /// Flux Sentry plants tripwire files (`.flux-sentry-tripwire.txt`) in the folders
    /// this backs up, and stops backing up when one is changed or removed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tripwires: bool,
    /// Where to tell how this backup goes, off this computer (besides the ones in
    /// Preferences, for every backup).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhooks: Vec<crate::webhook::Webhook>,
    /// The backups were made on another computer (or as another user, or under another
    /// id), and this profile carries them on: they're found, listed and continued as that
    /// computer's. Set when a backup is recovered on a new computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// Where a recovered backup was made: its backups are kept under these names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub host: String,
    pub user: String,
    /// The profile id there.
    pub profile: String,
}

/// The names a profile's backups are kept under at its target: computer, user, profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptAs {
    pub host: String,
    pub user: String,
    pub profile: String,
}

/// How readily changes count as unusual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Off,
    #[default]
    Normal,
    High,
}

impl Sensitivity {
    pub fn is_normal(&self) -> bool {
        *self == Sensitivity::Normal
    }
}

/// `[restore_test]`: how often a sample of the newest snapshot is read back and checked
/// (as it streams: nothing is written anywhere), and which sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreTest {
    #[serde(default)]
    pub every: TestEvery,
    #[serde(default)]
    pub mode: TestMode,
    /// `random_files`: how many.
    #[serde(default = "ten")]
    pub files: u32,
    /// `percent`: of the snapshot's size.
    #[serde(default = "five")]
    pub percent: u8,
    /// `confidence`: the share of damaged files a test must not miss (default 1%)…
    #[serde(default = "one_percent", skip_serializing_if = "is_one_percent")]
    pub damage: Percent,
    /// …and how sure it must be of catching it (default 99%). Together they set how many
    /// files are tested: see `flux_runner::sample`.
    #[serde(default = "ninety_nine", skip_serializing_if = "is_ninety_nine")]
    pub certainty: Percent,
    /// `rolling`: every file is tested once in this long (default 30 days).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<HumanDuration>,
    /// Stop after this long (the test then ends with a warning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime: Option<HumanDuration>,
    /// Stop after reading this much.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_read: Option<ByteSize>,
    /// Also compare each restored file with the file on disk now (differences are notes,
    /// not failures: files change).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compare_with_source: bool,
}

impl Default for RestoreTest {
    fn default() -> Self {
        Self {
            every: TestEvery::Off,
            mode: TestMode::RandomFiles,
            files: 10,
            percent: 5,
            damage: one_percent(),
            certainty: ninety_nine(),
            window: None,
            max_runtime: None,
            max_read: None,
            compare_with_source: false,
        }
    }
}

impl RestoreTest {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn ten() -> u32 {
    10
}

fn five() -> u8 {
    5
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

fn one_percent() -> Percent {
    Percent::from_millionths(10_000)
}

fn is_one_percent(p: &Percent) -> bool {
    *p == one_percent()
}

fn ninety_nine() -> Percent {
    Percent::from_millionths(990_000)
}

fn is_ninety_nine(p: &Percent) -> bool {
    *p == ninety_nine()
}

/// The least share of damaged files a restore test can be asked to catch, and the most:
/// 0.01% to 50%.
pub const DAMAGE_RANGE: (u32, u32) = (100, 500_000);
/// How sure it can be asked to be: 50% to 99.99%. (Never 100%: only testing every file is
/// certain, and that's `full`.)
pub const CERTAINTY_RANGE: (u32, u32) = (500_000, 999_900);

/// How often a restore test runs (after a backup, once it's due).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestEvery {
    /// Only by hand.
    #[default]
    Off,
    /// After every backup.
    Backup,
    Week,
    Month,
}

/// Which files a restore test takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestMode {
    #[default]
    RandomFiles,
    Percent,
    Full,
    Rolling,
    /// As many random files as it takes to catch damage to a given share of the files
    /// with a given certainty (`damage`, `certainty`).
    Confidence,
}

/// One more place a profile's backups go: same sources, schedule and retention, its own
/// target and engine. Its snapshots are those of profile id `<profile>+<name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    /// Short id, unique in the profile (`nas`, `usb`): lowercase letters, digits, `-`, `_`.
    pub name: String,
    /// Shown in the app ("NAS in the office"); defaults to the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub engine: Engine,
    pub target: Target,
    /// Kept away from this computer's building.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offsite: bool,
    /// How its copy is made: from the sources (independent), or copied from the main
    /// destination's backup (replicate: the sources are read once).
    #[serde(default, skip_serializing_if = "CopyMode::is_independent")]
    pub mode: CopyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsync: Option<RsyncOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borg: Option<BorgOptions>,
}

/// How a destination's copy is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyMode {
    /// Backed up from the sources, like the main destination: a fault in one copy never
    /// reaches another.
    #[default]
    Independent,
    /// The main destination's backup copied over after each run (the same snapshots, the
    /// same Vault and password): the sources are read once. Vault anywhere; Mirror and
    /// rsync between drives on this computer; never Borg.
    Replicate,
}

impl CopyMode {
    pub fn is_independent(&self) -> bool {
        *self == CopyMode::Independent
    }
}

impl Destination {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

impl Profile {
    /// The profile as seen by destination `name`: its target, engine and engine options,
    /// everything else the same, no further destinations, and the name "Home → nas".
    pub fn for_destination(&self, name: &str) -> Option<Profile> {
        let d = self.destinations.iter().find(|d| d.name == name)?;
        let mut p = self.clone();
        p.name = format!("{} → {}", self.name, d.label());
        p.engine = d.engine;
        p.target = d.target.clone();
        p.offsite = d.offsite;
        if d.mode == CopyMode::Independent {
            p.vault = d.vault.clone();
            p.rsync = d.rsync.clone();
            p.borg = d.borg.clone();
        }
        // A replica is the main destination's backup, engine options and password and all.
        p.replica = d.mode == CopyMode::Replicate;
        p.destinations = Vec::new();
        Some(p)
    }
}

/// Upkeep after scheduled backups: a full verification now and then, and reclaiming the
/// space of removed snapshots (Vault forgets them at each backup; the space comes back
/// when unused data is deleted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maintenance {
    /// Read every stored byte back about once a month.
    #[serde(default)]
    pub verify_monthly: bool,
    /// Vault: delete data no snapshot uses about once a week.
    #[serde(default = "yes")]
    pub free_space_weekly: bool,
}

impl Default for Maintenance {
    fn default() -> Self {
        Self {
            verify_monthly: false,
            free_space_weekly: true,
        }
    }
}

impl Maintenance {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
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
    /// Inside a git working tree (a folder with `.git`), also leave out what its
    /// `.gitignore` files ignore: build output, dependencies, and also files git leaves
    /// out on purpose, like `.env`. Mirror and Vault only; rsync and Borg don't read them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_gitignored: bool,
    /// Sources on a drive of their own (an external disk at `/mnt/photos`), recognised by
    /// filesystem UUID, and what to do when that drive isn't there. Recorded when such a
    /// source is added. A missing drive is never read as "everything was deleted".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<SourceVolume>,
}

/// A source on a drive of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceVolume {
    /// The source (one of `sources.paths`).
    pub path: PathBuf,
    /// The filesystem it must be on.
    pub fs_uuid: String,
    #[serde(default)]
    pub when_missing: WhenMissing,
    /// With `wait`: how long to wait at most (default 1 hour), then fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<HumanDuration>,
}

/// What a backup does when a source's drive isn't there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhenMissing {
    /// Back up the rest. Mirror and Vault keep the source's files from the previous
    /// snapshot; rsync and Borg leave it out of this one. Either way it's said.
    #[default]
    Skip,
    /// Wait for the drive (up to `wait_for`), then fail.
    Wait,
    /// Fail the backup.
    Fail,
}

impl WhenMissing {
    pub fn label(self) -> &'static str {
        match self {
            WhenMissing::Skip => "skip",
            WhenMissing::Wait => "wait",
            WhenMissing::Fail => "fail",
        }
    }
}

/// Which snapshots survive pruning. A snapshot is kept if *any* rule keeps it.
///
/// Bucket rules (`hourly` … `yearly`) keep the newest snapshot of each of the last N
/// periods that contain a snapshot, like restic and borg do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RetentionFile")]
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
    /// Pruning never leaves fewer than this many complete snapshots (no per-file
    /// problems), whatever the rules above say. 0: no floor.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub safety_floor: u32,
    /// What pruning does after a backup with problems (failed, files skipped, a source's
    /// drive missing, a destination failing).
    #[serde(default, skip_serializing_if = "AfterProblems::is_hold")]
    pub after_problems: AfterProblems,
    /// Run history (Activity) older than this many days is removed. `None`: kept up to
    /// the last 1000 runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_history_days: Option<u32>,
    /// At most this many runs of history are kept (at most 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_history_runs: Option<u32>,
}

/// [`Retention`] as written in a profile. The keep rules are told apart from "not
/// written": a `[retention]` table that sets none of them (only, say, `min_free_space` or
/// `keep_history_days`) keeps the default rules, the same as no table at all. It used to
/// read as "keep nothing but the newest". A table that sets any keep rule means exactly
/// what it says, the others 0.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionFile {
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    keep_last: Option<u32>,
    #[serde(default)]
    keep_within: Option<HumanDuration>,
    #[serde(default)]
    keep_hourly: Option<u32>,
    #[serde(default)]
    keep_daily: Option<u32>,
    #[serde(default)]
    keep_weekly: Option<u32>,
    #[serde(default)]
    keep_monthly: Option<u32>,
    #[serde(default)]
    keep_yearly: Option<u32>,
    #[serde(default)]
    max_age: Option<HumanDuration>,
    #[serde(default = "yes")]
    keep_named: bool,
    #[serde(default)]
    min_free_space: Option<ByteSize>,
    #[serde(default)]
    min_free_inodes_percent: Option<u8>,
    #[serde(default = "one")]
    safety_floor: u32,
    #[serde(default)]
    after_problems: AfterProblems,
    #[serde(default)]
    keep_history_days: Option<u32>,
    #[serde(default)]
    keep_history_runs: Option<u32>,
}

impl From<RetentionFile> for Retention {
    fn from(f: RetentionFile) -> Self {
        let rules_unset = f.keep_last.is_none()
            && f.keep_within.is_none()
            && f.keep_hourly.is_none()
            && f.keep_daily.is_none()
            && f.keep_weekly.is_none()
            && f.keep_monthly.is_none()
            && f.keep_yearly.is_none();
        let d = Retention::default();
        let rule = |written: Option<u32>, default: u32| {
            written.unwrap_or(if rules_unset { default } else { 0 })
        };
        Self {
            enabled: f.enabled,
            keep_last: rule(f.keep_last, d.keep_last),
            keep_within: if rules_unset {
                d.keep_within
            } else {
                f.keep_within
            },
            keep_hourly: rule(f.keep_hourly, d.keep_hourly),
            keep_daily: rule(f.keep_daily, d.keep_daily),
            keep_weekly: rule(f.keep_weekly, d.keep_weekly),
            keep_monthly: rule(f.keep_monthly, d.keep_monthly),
            keep_yearly: rule(f.keep_yearly, d.keep_yearly),
            max_age: f.max_age,
            keep_named: f.keep_named,
            min_free_space: f.min_free_space,
            min_free_inodes_percent: f.min_free_inodes_percent,
            safety_floor: f.safety_floor,
            after_problems: f.after_problems,
            keep_history_days: f.keep_history_days,
            keep_history_runs: f.keep_history_runs,
        }
    }
}

/// What scheduled pruning does after a backup with problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterProblems {
    /// Keep every snapshot until a backup runs cleanly, so a bad run can't push good
    /// snapshots out.
    #[default]
    Hold,
    /// Prune as usual.
    Prune,
}

impl AfterProblems {
    pub fn is_hold(&self) -> bool {
        *self == AfterProblems::Hold
    }
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
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
            safety_floor: 1,
            after_problems: AfterProblems::Hold,
            keep_history_days: None,
            keep_history_runs: None,
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
    /// Do not start scheduled backups to a server over a metered connection (as
    /// NetworkManager reports it: mobile data, a phone's hotspot…).
    #[serde(default)]
    pub skip_on_metered: bool,
    /// Also back up when the target drive (`target.fs_uuid`) is connected.
    #[serde(default)]
    pub on_connect: bool,
    /// When a backup ends, unmount its drive and power it off, so it's safe to unplug.
    /// Only for drives the desktop mounted (under `/run/media` or `/media`); others are
    /// left mounted, and the run says why.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub release_drive: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            trigger: Trigger::Manual,
            on_calendar: None,
            every: None,
            catch_up: true,
            skip_on_battery: false,
            skip_on_metered: false,
            on_connect: false,
            release_drive: false,
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
    /// How hard the password is hashed (Argon2id) for this Vault's new passwords: when it's
    /// created, and passwords added or changed. Existing passwords change with
    /// `flux key strengthen`.
    #[serde(default, skip_serializing_if = "PasswordHashing::is_standard")]
    pub hashing: PasswordHashing,
    /// Pad the files the Vault stores so their sizes say less about the files backed up.
    /// Costs a little space; applies to what's written from then on. On in new Vault
    /// profiles from 1.2 ([`VaultOptions::default`]); a profile file that doesn't say
    /// keeps it off, so existing backups don't change.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_sizes: bool,
    /// Keep repair data with what's stored: this share of it (percent, 1–50), as
    /// Reed–Solomon parity that can rebuild damaged parts (bad sectors, bit rot). 0: none.
    /// Applies to what's written from then on; verifying every byte adds it to the rest.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub parity: u8,
    /// The app locks the Vault again after this many minutes without use (forgets the
    /// password it was given). `None`: unlocked until the app closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_after_minutes: Option<u32>,
    /// An unlock file: the Vault's password works only together with this file (kept on a
    /// USB stick, for example). Set up with `flux key unlock-file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlock_file: Option<PathBuf>,
    /// Keep `flux-vault-recover` (a small program that reads Vaults without Flux) and a
    /// note on using it next to the Vault, on drives.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub keep_recovery_tool: bool,
}

impl Default for VaultOptions {
    fn default() -> Self {
        Self {
            compression_level: default_zstd(),
            encryption: true,
            password_source: PasswordSource::default(),
            hashing: PasswordHashing::default(),
            hide_sizes: true,
            parity: 0,
            lock_after_minutes: None,
            unlock_file: None,
            keep_recovery_tool: true,
        }
    }
}

/// How hard a Vault password is hashed (Argon2id): harder makes guessing a stolen drive's
/// password slower, and unlocking slower by the same factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "level")]
pub enum PasswordHashing {
    /// 64 MiB, 3 passes, 4 threads (Flux's default).
    #[default]
    Standard,
    /// 256 MiB, 4 passes, 4 threads.
    Strong,
    /// 1 GiB, 6 passes, 4 threads.
    Maximum,
    Custom {
        memory_mib: u32,
        passes: u32,
        threads: u32,
    },
}

impl PasswordHashing {
    pub const MEMORY_MIB: std::ops::RangeInclusive<u32> = 8..=4096;
    pub const PASSES: std::ops::RangeInclusive<u32> = 1..=20;
    pub const THREADS: std::ops::RangeInclusive<u32> = 1..=16;

    pub fn is_standard(&self) -> bool {
        *self == PasswordHashing::Standard
    }

    /// (memory in KiB, passes, threads).
    pub fn params(self) -> (u32, u32, u32) {
        match self {
            PasswordHashing::Standard => (64 * 1024, 3, 4),
            PasswordHashing::Strong => (256 * 1024, 4, 4),
            PasswordHashing::Maximum => (1024 * 1024, 6, 4),
            PasswordHashing::Custom {
                memory_mib,
                passes,
                threads,
            } => (memory_mib.saturating_mul(1024), passes, threads),
        }
    }

    /// The preset with these parameters, or `Custom`.
    pub fn from_params(m_kib: u32, t: u32, p: u32) -> Self {
        [Self::Standard, Self::Strong, Self::Maximum]
            .into_iter()
            .find(|h| h.params() == (m_kib, t, p))
            .unwrap_or(Self::Custom {
                memory_mib: m_kib / 1024,
                passes: t,
                threads: p,
            })
    }

    pub fn label(self) -> &'static str {
        match self {
            PasswordHashing::Standard => "standard",
            PasswordHashing::Strong => "strong",
            PasswordHashing::Maximum => "maximum",
            PasswordHashing::Custom { .. } => "custom",
        }
    }
}

fn default_zstd() -> u8 {
    3
}

/// rsync-engine settings. Only meaningful when `engine = "rsync"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RsyncOptions {
    /// Extra arguments added to every rsync backup command (e.g. `--compress` for slow
    /// links). They're recorded with each snapshot and in the recovery script.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

/// Borg-engine settings. Only meaningful when `engine = "borg"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BorgOptions {
    /// Fixed when the repository is created.
    #[serde(default)]
    pub encryption: BorgEncryption,
    /// Borg's `--compression` value: `lz4`, `zstd,3`, `auto,zstd,10`, `none`, …
    #[serde(default = "default_borg_compression")]
    pub compression: String,
    /// Where the passphrase comes from (not used when `encryption = "none"`).
    #[serde(default)]
    pub password_source: PasswordSource,
    /// The borg program on the SSH server, when it isn't `borg` on its PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_path: Option<String>,
    /// Extra arguments added to every `borg create`. Recorded in the recovery script.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

impl Default for BorgOptions {
    fn default() -> Self {
        Self {
            encryption: BorgEncryption::default(),
            compression: default_borg_compression(),
            password_source: PasswordSource::default(),
            remote_path: None,
            extra_args: Vec::new(),
        }
    }
}

fn default_borg_compression() -> String {
    "zstd,3".into()
}

/// Borg encryption modes Flux creates repositories with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BorgEncryption {
    /// Authenticated encryption, the key stored in the repository and protected by the
    /// passphrase (Borg's recommended mode).
    #[default]
    #[serde(rename = "repokey-blake2")]
    RepokeyBlake2,
    /// The same with SHA-256 (faster on CPUs with SHA extensions but slower BLAKE2).
    #[serde(rename = "repokey")]
    Repokey,
    /// No encryption and no authentication.
    #[serde(rename = "none")]
    None,
}

impl BorgEncryption {
    /// The value for `borg init --encryption`.
    pub fn as_arg(self) -> &'static str {
        match self {
            BorgEncryption::RepokeyBlake2 => "repokey-blake2",
            BorgEncryption::Repokey => "repokey",
            BorgEncryption::None => "none",
        }
    }
}

/// borg create options that would break Flux's reading of the repository.
/// A file size limit below this (1 MiB) leaves out nearly everything, which is almost
/// always a slip (a bare "1" means one byte): validation warns about it.
pub const TINY_SIZE_LIMIT: u64 = 1 << 20;

pub const BORG_RESERVED_ARGS: &[&str] = &[
    "--json",
    "--log-json",
    "--progress",
    "--list",
    "--filter",
    "--comment",
    "--dry-run",
    "-n",
    "--patterns-from",
    "--stdin-name",
    "--paths-from-stdin",
    "--paths-from-command",
    "--content-from-command",
    "--encryption",
    "-e",
];

/// rsync options that would break the snapshot layout or Flux's reading of it.
pub const RSYNC_RESERVED_ARGS: &[&str] = &[
    "--link-dest",
    "--compare-dest",
    "--copy-dest",
    "--relative",
    "-R",
    "--no-relative",
    "--files-from",
    "--backup",
    "-b",
    "--backup-dir",
    "--remove-source-files",
    "--dry-run",
    "-n",
    "--list-only",
    "--only-write-batch",
    "--write-batch",
    "--read-batch",
    "--inplace",
    "--fake-super",
];

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

fn is_true(b: &bool) -> bool {
    *b
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
                exclude_gitignored: false,
                volumes: Vec::new(),
            },
            retention: Retention::default(),
            schedule: Schedule::default(),
            options: Options::default(),
            hooks: Hooks::default(),
            vault: None,
            rsync: None,
            borg: None,
            maintenance: Maintenance::default(),
            offsite: false,
            destinations: Vec::new(),
            replica: false,
            network: Default::default(),
            restore_test: RestoreTest::default(),
            unusual_changes: Sensitivity::Normal,
            tripwires: false,
            webhooks: Vec::new(),
            origin: None,
        }
    }

    /// The names this profile's backups are kept under: this computer, user and id, or
    /// the [`Origin`] they were made as (a place added to it keeps its `+name`).
    pub fn kept_as(&self, id: &ProfileId) -> KeptAs {
        match &self.origin {
            None => KeptAs {
                host: crate::sys::hostname(),
                user: crate::sys::username(),
                profile: id.to_string(),
            },
            Some(o) => KeptAs {
                host: o.host.clone(),
                user: o.user.clone(),
                profile: match id.split_destination() {
                    Some((_, dest)) if !o.profile.contains(DESTINATION_SEP) => {
                        format!("{}{DESTINATION_SEP}{dest}", o.profile)
                    }
                    _ => o.profile.clone(),
                },
            },
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Does it start backups by itself (on a schedule, or when its drive is plugged in)?
    pub fn runs_on_its_own(&self) -> bool {
        self.schedule.trigger != Trigger::Manual || self.schedule.on_connect
    }

    /// Where this profile's password comes from, or `None` when it needs none (Mirror,
    /// rsync, unencrypted Vault or Borg).
    pub fn password_source(&self) -> Option<PasswordSource> {
        match self.engine {
            Engine::Vault => self.vault.as_ref().is_none_or(|v| v.encryption).then(|| {
                self.vault
                    .as_ref()
                    .map(|v| v.password_source.clone())
                    .unwrap_or_default()
            }),
            Engine::Borg => {
                let b = self.borg.clone().unwrap_or_default();
                (b.encryption != BorgEncryption::None).then_some(b.password_source)
            }
            Engine::Mirror | Engine::Rsync => None,
        }
    }

    /// [`Profile::validate`] plus what only matters in `scope`: system backups run without
    /// anyone logged in, so their Vault password must come from a file or a command; a
    /// user's backups that start by themselves can't ask for one either.
    pub fn validate_in(&self, scope: crate::Scope) -> Vec<Issue> {
        let mut issues = self.validate();
        let password_field = || {
            if self.engine == Engine::Borg {
                "borg.password_source".to_owned()
            } else {
                "vault.password_source".to_owned()
            }
        };
        if scope == crate::Scope::User
            && self.runs_on_its_own()
            && matches!(self.password_source(), Some(PasswordSource::Prompt))
        {
            issues.push(Issue::warning(
                &password_field(),
                "scheduled backups will fail: the password is set to \"Ask every time\", and nobody is there to type it when a backup starts on its own. Keep it in the desktop keyring, a key file or a command".into(),
            ));
        }
        if scope == crate::Scope::System
            && matches!(
                self.password_source(),
                Some(PasswordSource::Keyring | PasswordSource::Prompt)
            )
        {
            issues.push(Issue {
                severity: Severity::Warning,
                field: password_field(),
                message: "system backups run without anyone logged in, so they can't use a keyring or ask for the password: keep it in a key file only root can read (/etc/flux-backup/keys/<id>)".into(),
            });
        }
        // Plain folders on a FAT32, exFAT or NTFS drive (when it's mounted).
        if let Some(fit) = crate::target_check::filesystem_fit_of(self) {
            issues.push(Issue::warning("target.path", fit.sentence()));
        }
        issues
    }

    /// Check the profile for problems. Errors make the profile unusable; warnings do not.
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
        if self.name.chars().any(char::is_control) {
            err(
                "name",
                "name must be one line of plain text (it has a line break or another control character)"
                    .into(),
            );
        }

        match &self.target {
            Target::Local { path, fs_uuid } => {
                if path.as_os_str().is_empty() {
                    // A new profile starts without one; the app shows this as "To continue: …".
                    err("target.path", "choose a drive or folder".into());
                } else if !path.is_absolute() {
                    err(
                        "target.path",
                        format!("{} is not an absolute path", path.display()),
                    );
                }
                if fs_uuid.as_deref().is_some_and(|u| u.trim().is_empty()) {
                    err(
                        "target.fs_uuid",
                        "the drive's id is empty: choose the drive again while it's connected"
                            .into(),
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

        // Mirror over SSH sends files with rsync, which there stores a link as a link
        // whatever this says: the snapshot would list a file and hold a link to a path on
        // this computer, and a restore would find nothing.
        if self.engine == Engine::Mirror
            && matches!(self.target, Target::Ssh { .. })
            && self.options.symlinks != SymlinkMode::Preserve
        {
            err(
                "options.symlinks",
                "a Mirror on an SSH server stores links as links; to copy what links point to, choose the Vault or rsync format, or a Mirror on a drive".into(),
            );
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
                    "choose the times it runs (for example: daily, or Mon..Fri 18:00)".into(),
                ));
            }
            Trigger::Interval if sch.every.is_none_or(|e| e.0.as_secs() < 60) => {
                v.push(Issue::error(
                    "schedule.every",
                    "the time between backups must be at least a minute".into(),
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
                "backing up when the drive is plugged in needs a drive Flux knows: choose it while it's connected".into(),
            ));
        }
        if self.schedule.release_drive
            && !matches!(&self.target, Target::Local { .. })
            && !self
                .destinations
                .iter()
                .any(|d| matches!(d.target, Target::Local { .. }))
        {
            v.push(Issue::warning(
                "schedule.release_drive",
                "unmounting the drive when done only applies to backups to a drive".into(),
            ));
        }

        if let Some(o) = &self.origin
            && [&o.host, &o.user, &o.profile]
                .iter()
                .any(|s| s.is_empty() || s.contains('/') || s.starts_with('.'))
        {
            v.push(Issue::error(
                "origin",
                "the computer, user and profile a backup was made as are plain names".into(),
            ));
        }
        for (i, w) in self.webhooks.iter().enumerate() {
            for msg in w.problems() {
                v.push(Issue::error(&format!("webhooks[{i}]"), msg));
            }
        }
        if self.restore_test.percent == 0 || self.restore_test.percent > 100 {
            v.push(Issue::error(
                "restore_test.percent",
                "a restore test takes 1–100 percent".into(),
            ));
        }
        let dmg = self.restore_test.damage.millionths();
        if !(DAMAGE_RANGE.0..=DAMAGE_RANGE.1).contains(&dmg) {
            v.push(Issue::error(
                "restore_test.damage",
                "a restore test can catch damage to between 0.01% and 50% of the files".into(),
            ));
        }
        let sure = self.restore_test.certainty.millionths();
        if !(CERTAINTY_RANGE.0..=CERTAINTY_RANGE.1).contains(&sure) {
            v.push(Issue::error(
                "restore_test.certainty",
                "a restore test's certainty is between 50% and 99.99% (to be certain, test everything)".into(),
            ));
        }
        for (i, msg) in self.network.problems() {
            v.push(Issue::error(&format!("network.bandwidth[{i}]"), msg));
        }
        for vol in &self.sources.volumes {
            if !self.sources.paths.contains(&vol.path) {
                v.push(Issue::warning(
                    "sources.volumes",
                    format!(
                        "{} has a drive recorded but isn't a source; the record is ignored",
                        vol.path.display()
                    ),
                ));
            }
            if vol.fs_uuid.trim().is_empty() {
                v.push(Issue::error(
                    "sources.volumes",
                    format!(
                        "the drive of {} needs its filesystem UUID",
                        vol.path.display()
                    ),
                ));
            }
            if vol.wait_for.is_some() && vol.when_missing != WhenMissing::Wait {
                v.push(Issue::warning(
                    "sources.volumes",
                    format!(
                        "wait_for only applies with when_missing = \"wait\" ({})",
                        vol.path.display()
                    ),
                ));
            }
        }
        if self.options.nice > 19 {
            v.push(Issue::error("options.nice", "nice must be 0-19".into()));
        }

        match (self.engine, &self.vault) {
            (Engine::Vault, Some(vo)) => {
                if vo.compression_level > 22 {
                    v.push(Issue::error(
                        "vault.compression_level",
                        "zstd level must be 0-22".into(),
                    ));
                }
                if vo.parity > 50 {
                    v.push(Issue::error(
                        "vault.parity",
                        "repair data takes 1–50 percent (0 for none)".into(),
                    ));
                }
                if let PasswordHashing::Custom {
                    memory_mib,
                    passes,
                    threads,
                } = vo.hashing
                {
                    use PasswordHashing as H;
                    if !H::MEMORY_MIB.contains(&memory_mib)
                        || !H::PASSES.contains(&passes)
                        || !H::THREADS.contains(&threads)
                    {
                        v.push(Issue::error(
                            "vault.hashing",
                            format!(
                                "password hashing: memory {}-{} MiB, {}-{} passes, {}-{} threads",
                                H::MEMORY_MIB.start(),
                                H::MEMORY_MIB.end(),
                                H::PASSES.start(),
                                H::PASSES.end(),
                                H::THREADS.start(),
                                H::THREADS.end()
                            ),
                        ));
                    }
                }
                if !vo.encryption && (vo.hide_sizes || vo.unlock_file.is_some()) {
                    v.push(Issue::warning(
                        "vault.encryption",
                        "hiding sizes and unlock files only apply to encrypted Vaults".into(),
                    ));
                }
                if let Some(f) = &vo.unlock_file
                    && !f.is_absolute()
                {
                    v.push(Issue::error(
                        "vault.unlock_file",
                        "the unlock file needs a full path".into(),
                    ));
                }
                if vo.lock_after_minutes == Some(0) {
                    v.push(Issue::error(
                        "vault.lock_after_minutes",
                        "lock again after at least 1 minute".into(),
                    ));
                }
            }
            (Engine::Mirror | Engine::Rsync | Engine::Borg, Some(_)) => v.push(Issue::warning(
                "vault",
                format!("[vault] settings are ignored for {} profiles", self.engine),
            )),
            _ => {}
        }
        if self.engine != Engine::Rsync && self.rsync.is_some() {
            v.push(Issue::warning(
                "rsync",
                format!("[rsync] settings are ignored for {} profiles", self.engine),
            ));
        }
        if self.engine != Engine::Borg && self.borg.is_some() {
            v.push(Issue::warning(
                "borg",
                format!("[borg] settings are ignored for {} profiles", self.engine),
            ));
        }
        if let Some(max) = self.sources.exclude_larger_than
            && self.engine != Engine::Borg
            && max.bytes() < TINY_SIZE_LIMIT
        {
            v.push(Issue::warning(
                "sources.exclude_larger_than",
                format!(
                    "every file larger than {max} is left out, which is almost every file; \
                     a limit like 1 GB or 4 GB is usual"
                ),
            ));
        }
        if self.sources.exclude_gitignored && matches!(self.engine, Engine::Rsync | Engine::Borg) {
            v.push(Issue::warning(
                "sources.exclude_gitignored",
                format!(
                    "{} doesn't read .gitignore files, so what they ignore is backed up anyway",
                    if self.engine == Engine::Borg {
                        "Borg"
                    } else {
                        "rsync"
                    }
                ),
            ));
        }
        if self.engine == Engine::Borg {
            let b = self.borg.clone().unwrap_or_default();
            if b.compression.trim().is_empty() || b.compression.contains(char::is_whitespace) {
                v.push(Issue::error(
                    "borg.compression",
                    "use a Borg compression such as lz4, zstd,3 or none".into(),
                ));
            }
            if self.sources.exclude_larger_than.is_some() {
                v.push(Issue::warning(
                    "sources.exclude_larger_than",
                    "Borg can't leave out files by size; this limit is ignored".into(),
                ));
            }
            if self.options.symlinks != SymlinkMode::Preserve {
                v.push(Issue::warning(
                    "options.symlinks",
                    "Borg always stores symbolic links as links; this setting is ignored".into(),
                ));
            }
            for (i, arg) in b.extra_args.iter().enumerate() {
                let name = arg.split('=').next().unwrap_or(arg);
                if arg.trim().is_empty() || !arg.starts_with('-') {
                    v.push(Issue::error(
                        &format!("borg.extra_args[{i}]"),
                        format!("{arg:?} is not an option (options start with -)"),
                    ));
                } else if BORG_RESERVED_ARGS.contains(&name) {
                    v.push(Issue::error(
                        &format!("borg.extra_args[{i}]"),
                        format!("{name} is set by Flux and can't be changed here"),
                    ));
                }
            }
        }
        if self.engine == Engine::Rsync {
            for (i, arg) in self
                .rsync
                .iter()
                .flat_map(|r| r.extra_args.iter())
                .enumerate()
            {
                let name = arg.split('=').next().unwrap_or(arg);
                if arg.trim().is_empty() {
                    v.push(Issue::error(
                        &format!("rsync.extra_args[{i}]"),
                        "empty argument".into(),
                    ));
                } else if RSYNC_RESERVED_ARGS.contains(&name) {
                    v.push(Issue::error(
                        &format!("rsync.extra_args[{i}]"),
                        format!("{name} is set by Flux and can't be changed here"),
                    ));
                } else if !arg.starts_with('-') {
                    v.push(Issue::error(
                        &format!("rsync.extra_args[{i}]"),
                        format!("{arg:?} is not an option (options start with -)"),
                    ));
                }
            }
        }

        // More destinations: valid, distinct names; somewhere other than the others; and
        // each fine as the profile it becomes.
        let mut seen: Vec<&str> = Vec::new();
        for (i, d) in self.destinations.iter().enumerate() {
            let field = format!("destinations[{i}]");
            if ProfileId::new(format!("x{DESTINATION_SEP}{}", d.name)).is_err() {
                v.push(Issue::error(
                    &format!("{field}.name"),
                    format!(
                        "destination name {:?}: use lowercase letters, digits, - and _",
                        d.name
                    ),
                ));
            } else if seen.contains(&d.name.as_str()) {
                v.push(Issue::error(
                    &format!("{field}.name"),
                    format!("two destinations are named {:?}", d.name),
                ));
            }
            seen.push(&d.name);
            let same_place = |t: &Target| match (t, &d.target) {
                (Target::Local { path: a, .. }, Target::Local { path: b, .. }) => a == b,
                (
                    Target::Ssh {
                        host: h1, path: p1, ..
                    },
                    Target::Ssh {
                        host: h2, path: p2, ..
                    },
                ) => h1 == h2 && p1 == p2,
                _ => false,
            };
            if (same_place(&self.target) && self.engine == d.engine)
                || self.destinations[..i]
                    .iter()
                    .any(|o| same_place(&o.target) && o.engine == d.engine)
            {
                v.push(Issue::error(
                    &format!("{field}.target"),
                    format!(
                        "{} goes to the same place as another destination",
                        d.label()
                    ),
                ));
            }
            if d.mode == CopyMode::Replicate {
                let why = if d.engine != self.engine {
                    Some(format!(
                        "a copy of the main backup is stored the same way ({})",
                        self.engine.label()
                    ))
                } else if self.engine == Engine::Borg {
                    Some("Borg repositories can't be copied safely; back up to it from the sources (independent)".into())
                } else if self.engine != Engine::Vault
                    && !(matches!(self.target, Target::Local { .. })
                        && matches!(d.target, Target::Local { .. }))
                {
                    Some("plain-folder backups are copied between drives on this computer only; use a Vault, or back up from the sources".into())
                } else {
                    None
                };
                if let Some(why) = why {
                    v.push(Issue::error(
                        &format!("{field}.mode"),
                        format!("{}: {why}", d.label()),
                    ));
                }
            }
            if let Some(as_profile) = self.for_destination(&d.name) {
                for issue in as_profile.validate() {
                    let f = issue.field.clone();
                    if ["engine", "target", "vault", "rsync", "borg"]
                        .iter()
                        .any(|k| f == *k || f.starts_with(&format!("{k}.")))
                    {
                        v.push(Issue {
                            field: format!("{field}.{f}"),
                            message: format!("{}: {}", d.label(), issue.message),
                            ..issue
                        });
                    }
                }
            }
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
    fn tiny_size_limit_is_warned_about() {
        let mut p = sample();
        p.sources.exclude_larger_than = Some(ByteSize(1));
        let issues = p.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.field == "sources.exclude_larger_than" && i.message.contains("1B")),
            "{issues:?}"
        );
        p.sources.exclude_larger_than = Some(ByteSize(4 << 30));
        assert!(p.validate().is_empty(), "{:?}", p.validate());
    }

    #[test]
    fn a_missing_destination_says_what_to_do() {
        let mut p = sample();
        p.target = Target::Local {
            path: PathBuf::new(),
            fs_uuid: None,
        };
        let issues = p.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.field == "target.path" && i.message == "choose a drive or folder"),
            "{issues:?}"
        );
    }

    #[test]
    fn default_profile_is_valid_and_round_trips() {
        let p = sample();
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        let s = p.to_toml().unwrap();
        assert_eq!(Profile::from_toml(&s).unwrap(), p);
    }

    /// A `[retention]` table that sets no keep rule keeps the default ones (it used to
    /// mean "keep only the newest"); one that sets any means what it says.
    #[test]
    fn a_retention_table_without_keep_rules_keeps_the_defaults() {
        let with = |retention: &str| {
            Profile::from_toml(&format!(
                "name = \"Docs\"\n[target]\nkind = \"local\"\npath = \"/mnt/b\"\n\
                 [sources]\npaths = [\"/home/me\"]\n[retention]\n{retention}\n"
            ))
            .unwrap()
            .retention
        };
        let r = with("keep_history_days = 90\nmin_free_space = \"50GB\"");
        let d = Retention::default();
        assert_eq!(
            (
                r.keep_last,
                r.keep_within,
                r.keep_daily,
                r.keep_weekly,
                r.keep_monthly,
                r.keep_yearly
            ),
            (
                d.keep_last,
                d.keep_within,
                d.keep_daily,
                d.keep_weekly,
                d.keep_monthly,
                d.keep_yearly
            )
        );
        assert_eq!(r.keep_history_days, Some(90));
        assert!(r.min_free_space.is_some());
        // Any rule written: the others are 0, as before.
        let r = with("keep_daily = 3");
        assert_eq!(
            (r.keep_last, r.keep_within, r.keep_daily, r.keep_weekly),
            (0, None, 3, 0)
        );
        // What Flux writes reads back the same, `keep_within` left out included.
        let exact = Retention {
            keep_within: None,
            keep_daily: 0,
            ..Retention::default()
        };
        let text = toml::to_string(&exact).unwrap();
        assert_eq!(toml::from_str::<Retention>(&text).unwrap(), exact);
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
    fn new_vaults_hide_sizes_but_existing_profiles_keep_theirs() {
        // A Vault profile made from 1.2 pads its packs, and says so in its file.
        assert!(VaultOptions::default().hide_sizes);
        let mut p = Profile::new(
            "v",
            Target::Local {
                path: "/mnt".into(),
                fs_uuid: None,
            },
            vec![],
        );
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions::default());
        let text = p.to_toml().unwrap();
        assert!(text.contains("hide_sizes = true"), "{text}");
        assert!(Profile::from_toml(&text).unwrap().vault.unwrap().hide_sizes);

        // A file from before 1.2 doesn't name it, and stays as it was: off.
        let old = Profile::from_toml(
            r#"
            name = "x"
            engine = "vault"
            [target]
            kind = "local"
            path = "/mnt"
            [sources]
            paths = ["/home"]
            [vault]
            compression_level = 3
            "#,
        )
        .unwrap();
        assert!(!old.vault.unwrap().hide_sizes);
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
            safety_floor: 0,
            after_problems: crate::profile::AfterProblems::Hold,
            keep_history_days: None,
            keep_history_runs: None,
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
    fn rsync_extra_args_are_checked() {
        let mut p = sample();
        p.engine = Engine::Rsync;
        p.rsync = Some(RsyncOptions {
            extra_args: vec![
                "--compress".into(),
                "--link-dest=/x".into(),
                "-n".into(),
                "oops".into(),
            ],
        });
        let errors: Vec<_> = p
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.field)
            .collect();
        assert_eq!(
            errors,
            [
                "rsync.extra_args[1]",
                "rsync.extra_args[2]",
                "rsync.extra_args[3]"
            ]
        );
        let s = p.to_toml().unwrap();
        assert!(s.contains("engine = \"rsync\""));
        assert_eq!(Profile::from_toml(&s).unwrap(), p);
        p.engine = Engine::Mirror;
        assert!(p.validate().iter().any(|i| i.field == "rsync"));
    }

    #[test]
    fn warns_when_a_backup_on_its_own_would_ask_for_the_password() {
        let warned = |p: &Profile, scope| {
            p.validate_in(scope)
                .iter()
                .filter(|i| i.field == "vault.password_source")
                .count()
        };
        let mut p = sample();
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions {
            password_source: PasswordSource::Prompt,
            ..VaultOptions::default()
        });
        p.schedule.trigger = Trigger::Manual;
        assert_eq!(warned(&p, crate::Scope::User), 0, "by hand: it can ask");
        p.schedule.on_connect = true;
        assert_eq!(
            warned(&p, crate::Scope::User),
            1,
            "when the drive is plugged in"
        );
        p.schedule.on_connect = false;
        p.schedule.trigger = Trigger::Calendar;
        assert_eq!(warned(&p, crate::Scope::User), 1, "on a schedule");
        assert_eq!(
            warned(&p, crate::Scope::System),
            1,
            "system's own warning only"
        );
        p.vault.as_mut().unwrap().password_source = PasswordSource::Keyring;
        assert_eq!(
            warned(&p, crate::Scope::User),
            0,
            "the keyring works unattended"
        );
        p.vault.as_mut().unwrap().password_source = PasswordSource::Prompt;
        p.vault.as_mut().unwrap().encryption = false;
        assert_eq!(warned(&p, crate::Scope::User), 0, "no password at all");
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
