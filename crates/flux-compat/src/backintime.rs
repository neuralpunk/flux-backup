//! Settings from Back In Time (`~/.config/backintime/config`), as Flux profiles.
//!
//! Back In Time keeps every profile in one `key=value` file (`profile1.snapshots.path=…`).
//! Each profile becomes a Flux profile with the same folders, exclusions, schedule, what
//! to keep and options. Back In Time's snapshots aren't converted or touched: Flux starts
//! its own backups (by default with rsync, which writes the same kind of plain,
//! hard-linked folders). What can't be carried over exactly is listed in `notes`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use flux_core::profile::{
    ChangeDetection, Engine, IoPriority, Profile, RsyncOptions, SymlinkMode, Target, Trigger,
    VaultOptions,
};
use flux_core::{ByteSize, HumanDuration};

/// Where Back In Time keeps its settings, under the home folder.
pub const CONFIG: &str = ".config/backintime/config";

const DAY: u64 = 86_400;

/// One Back In Time profile, ready to become a Flux profile.
#[derive(Debug, Clone, Serialize)]
pub struct BitProfile {
    /// Back In Time's number for it (`profile1` → 1).
    pub number: u32,
    pub name: String,
    /// How Back In Time stores it: `local`, `local_encfs`, `ssh` or `ssh_encfs`.
    pub mode: String,
    /// The Flux profile with its settings (on top of the defaults it was given).
    pub profile: Profile,
    /// What didn't carry over exactly, in words.
    pub notes: Vec<String>,
    /// Where its snapshots are, for local profiles.
    pub snapshots_dir: Option<PathBuf>,
    /// Snapshots there now (local profiles; 0 when the drive isn't connected).
    pub snapshot_count: usize,
    /// The newest one's name (`20261101-070001-123`).
    pub latest: Option<String>,
    /// The destination folder is there now (local profiles: the drive is connected).
    pub connected: bool,
}

/// Back In Time's profiles from the config file at `path`, each applied to a copy of
/// `base` (a new Flux profile's defaults). `Ok(empty)` when the file doesn't exist.
pub fn read(path: &Path, base: &Profile) -> std::io::Result<Vec<BitProfile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse(&text, base, &|dir| scan_snapshots(dir))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The snapshots in a Back In Time snapshot folder: their count and the newest name.
type Scan<'a> = &'a dyn Fn(&Path) -> (usize, Option<String>);

fn scan_snapshots(dir: &Path) -> (usize, Option<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (0, None);
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| is_snapshot_name(n))
        .collect();
    names.sort();
    (names.len(), names.pop())
}

/// `20261101-070001-123`: date, time, a random number.
fn is_snapshot_name(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() >= 19
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'-'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'-'
        && b[16..].iter().all(u8::is_ascii_digit)
}

/// The profiles in config text (see [`read`]).
pub fn parse(text: &str, base: &Profile, scan: Scan<'_>) -> Vec<BitProfile> {
    let kv: HashMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let numbers: Vec<u32> = kv
        .get("profiles")
        .unwrap_or(&"1")
        .split(':')
        .filter_map(|n| n.trim().parse().ok())
        .collect();
    numbers
        .into_iter()
        .map(|n| convert(n, &Keys { kv: &kv, n }, base, scan))
        .collect()
}

/// One profile's keys (`profileN.<key>`).
struct Keys<'a> {
    kv: &'a HashMap<&'a str, &'a str>,
    n: u32,
}

impl Keys<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.kv
            .get(format!("profile{}.{key}", self.n).as_str())
            .copied()
            .filter(|v| !v.is_empty())
    }
    fn str(&self, key: &str) -> String {
        self.get(key).unwrap_or_default().to_owned()
    }
    fn bool(&self, key: &str, default: bool) -> bool {
        self.get(key).map_or(default, |v| v == "true")
    }
    fn num(&self, key: &str, default: u64) -> u64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    /// A numbered list: `<key>.1.value`, `<key>.2.value`… (`<key>.size` of them).
    fn list(&self, key: &str) -> Vec<(String, u64)> {
        (1..=self.num(&format!("{key}.size"), 0))
            .filter_map(|i| {
                let v = self.get(&format!("{key}.{i}.value"))?;
                Some((v.to_owned(), self.num(&format!("{key}.{i}.type"), 0)))
            })
            .collect()
    }
}

fn convert(number: u32, k: &Keys<'_>, base: &Profile, scan: Scan<'_>) -> BitProfile {
    let mut p = base.clone();
    let mut notes = Vec::new();
    let name = k.get("name").map_or_else(
        || {
            if number == 1 {
                "Main profile".to_owned()
            } else {
                format!("Profile {number}")
            }
        },
        str::to_owned,
    );
    p.name = name.clone();

    // What to back up.
    p.sources.paths = k
        .list("snapshots.include")
        .into_iter()
        .map(|(v, _)| PathBuf::from(v))
        .collect();
    let mut changed = Vec::new();
    p.sources.exclude = k
        .list("snapshots.exclude")
        .into_iter()
        .filter_map(|(v, _)| {
            let c = pattern(&v)?;
            if c != v {
                changed.push(format!("{v} → {c}"));
            }
            Some(c)
        })
        .collect();
    if !changed.is_empty() {
        notes.push(format!(
            "Some exclusions are written differently in Flux, with the same meaning: {}.",
            changed.join(", ")
        ));
    }
    p.sources.exclude_larger_than = k
        .bool("snapshots.exclude.bysize.enabled", false)
        .then(|| ByteSize(k.num("snapshots.exclude.bysize.value", 500) << 20));
    p.sources.one_file_system = k.bool("snapshots.one_file_system", false);

    // Where.
    let mode = k.get("snapshots.mode").unwrap_or("local").to_owned();
    let mut snapshots_dir = None;
    match mode.as_str() {
        "ssh" | "ssh_encfs" => {
            let path = k.str("snapshots.ssh.path");
            p.target = Target::Ssh {
                host: k.str("snapshots.ssh.host"),
                user: k.get("snapshots.ssh.user").map(str::to_owned),
                port: k
                    .get("snapshots.ssh.port")
                    .and_then(|v| v.parse().ok())
                    .filter(|&port| port != 22),
                path: PathBuf::from(if path.is_empty() { "backups" } else { &path }),
                identity_file: k.get("snapshots.ssh.private_key_file").map(PathBuf::from),
                ssh_options: Vec::new(),
                bandwidth_limit: k
                    .bool("snapshots.bwlimit.enabled", false)
                    .then(|| ByteSize(k.num("snapshots.bwlimit.value", 0) * 1024)),
            };
            if !k.str("snapshots.ssh.proxy_host").is_empty() {
                notes.push(
                    "Back In Time reached the server through a proxy host; set that up in your \
                     ssh config (ProxyJump), which Flux uses."
                        .into(),
                );
            }
        }
        _ => {
            let key = if mode == "local_encfs" {
                "snapshots.local_encfs.path"
            } else {
                "snapshots.path"
            };
            let path = PathBuf::from(k.str(key));
            snapshots_dir = (!path.as_os_str().is_empty()).then(|| {
                path.join("backintime")
                    .join(k.get("snapshots.path.host").unwrap_or("localhost"))
                    .join(k.get("snapshots.path.user").unwrap_or("root"))
                    .join(k.get("snapshots.path.profile").unwrap_or("1"))
            });
            p.target = Target::Local {
                path,
                fs_uuid: None,
            };
        }
    }

    // How: plain hard-linked folders by rsync, like Back In Time made. Its EncFS
    // encryption has no equivalent; an encrypted Vault is the nearest.
    if mode.ends_with("_encfs") {
        p.engine = Engine::Vault;
        p.vault = Some(VaultOptions::default());
        notes.push(
            "Back In Time encrypted these backups with EncFS, which Flux doesn't use. They'll \
             be an encrypted Vault instead: choose its password on the Protection step."
                .into(),
        );
    } else {
        p.engine = Engine::Rsync;
        p.vault = None;
    }
    // Extra rsync options carry over to the rsync engine, except those Flux sets itself.
    let extra = k.str("snapshots.rsync_options.value");
    if k.bool("snapshots.rsync_options.enabled", false) && !extra.trim().is_empty() {
        let args = split_args(&extra);
        if p.engine == Engine::Rsync {
            let (kept, reserved): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| {
                let name = a.split('=').next().unwrap_or(a);
                a.starts_with('-') && !flux_core::profile::RSYNC_RESERVED_ARGS.contains(&name)
            });
            if !kept.is_empty() {
                p.rsync = Some(RsyncOptions { extra_args: kept });
            }
            if !reserved.is_empty() {
                notes.push(format!(
                    "These extra rsync options aren't carried over, because Flux sets them itself or they aren't options: {}.",
                    reserved.join(" ")
                ));
            }
        } else {
            notes.push(format!(
                "Back In Time passed extra rsync options ({extra}); an encrypted Vault doesn't use rsync, so they aren't carried over."
            ));
        }
    }

    // When.
    schedule(&mut p, k, &mut notes);
    p.schedule.skip_on_battery = k.bool("snapshots.no_on_battery", false);

    // What to keep.
    retention(&mut p, k);

    // Options.
    let o = &mut p.options;
    o.preserve_acl = k.bool("snapshots.preserve_acl", false);
    o.preserve_xattr = k.bool("snapshots.preserve_xattr", false);
    o.continue_on_errors = k.bool("snapshots.continue_on_errors", true);
    o.backup_before_restore = k.bool("snapshots.backup_on_restore.enabled", true);
    o.notify = k.bool("snapshots.notify.enabled", true);
    o.snapshot_if_unchanged = k.bool("snapshots.take_snapshot_regardless_of_changes", false);
    if k.bool("snapshots.use_checksum", false) {
        o.change_detection = ChangeDetection::Checksum;
    }
    o.symlinks = if k.bool("snapshots.copy_links", false) {
        SymlinkMode::Follow
    } else if k.bool("snapshots.copy_unsafe_links", false) {
        SymlinkMode::FollowUnsafe
    } else {
        SymlinkMode::Preserve
    };
    if !k.bool("snapshots.cron.ionice", true) {
        o.io_priority = IoPriority::Normal;
    }

    let (snapshot_count, latest) = snapshots_dir.as_deref().map(scan).unwrap_or_default();
    let connected = match &p.target {
        Target::Local { path, .. } => !path.as_os_str().is_empty() && path.is_dir(),
        Target::Ssh { .. } => true,
    };
    BitProfile {
        connected,
        number,
        name,
        mode,
        profile: p,
        notes,
        snapshots_dir,
        snapshot_count,
        latest,
    }
}

/// Back In Time's schedule modes (its `config.py`).
fn schedule(p: &mut Profile, k: &Keys<'_>, notes: &mut Vec<String>) {
    let s = &mut p.schedule;
    let hhmm = k.num("schedule.time", 0);
    let at = format!("{:02}:{:02}:00", (hhmm / 100) % 24, (hhmm % 100) % 60);
    let mut calendar = |c: String| {
        s.trigger = Trigger::Calendar;
        s.on_calendar = Some(c);
        s.every = None;
    };
    match k.num("schedule.mode", 0) {
        0 => {
            s.trigger = Trigger::Manual;
            s.on_calendar = None;
        }
        1 => {
            calendar("daily".into());
            notes.push(
                "Back In Time ran this at every start of the computer; Flux backs up daily \
                 instead, and catches up when the computer was off."
                    .into(),
            );
        }
        10 => calendar("hourly".into()),
        19 => {
            let hours = k.str("schedule.custom_time");
            let ok = !hours.is_empty() && hours.split(',').all(|h| h.trim().parse::<u8>().is_ok());
            calendar(if ok {
                format!("*-*-* {}:00:00", hours.replace(' ', ""))
            } else {
                "daily".into()
            });
        }
        20 => calendar(format!("*-*-* {at}")),
        27 => {
            s.trigger = Trigger::Manual;
            s.on_calendar = None;
            notes.push(
                "Back In Time backed up when the drive was connected. Pick the drive on the \
                 Where step, then turn on \u{201c}Also when this drive is plugged in\u{201d}."
                    .into(),
            );
        }
        30 => {
            let day = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                [(k.num("schedule.weekday", 7).clamp(1, 7) - 1) as usize];
            calendar(format!("{day} *-*-* {at}"));
        }
        40 => calendar(format!(
            "*-*-{:02} {at}",
            k.num("schedule.day", 1).clamp(1, 28)
        )),
        80 => calendar(format!("*-01-01 {at}")),
        mode => {
            let every = match mode {
                2 => Some(5 * 60),
                4 => Some(10 * 60),
                7 => Some(30 * 60),
                12 => Some(2 * 3600),
                14 => Some(4 * 3600),
                16 => Some(6 * 3600),
                18 => Some(12 * 3600),
                25 => Some(
                    duration(
                        k.num("schedule.repeatedly.period", 1).max(1),
                        k.num("schedule.repeatedly.unit", 20),
                    )
                    .0
                    .as_secs(),
                ),
                _ => None,
            };
            match every {
                Some(secs) => {
                    s.trigger = Trigger::Interval;
                    s.every = Some(HumanDuration::from_secs(secs));
                    s.on_calendar = None;
                }
                None => {
                    calendar("daily".into());
                    notes.push(format!(
                        "Back In Time's schedule (mode {mode}) has no match in Flux; it's set \
                         to daily."
                    ));
                }
            }
        }
    }
}

/// Back In Time's time units: hour 10, day 20, week 30, month 40, year 80.
fn unit_secs(unit: u64) -> u64 {
    match unit {
        10 => 3600,
        30 => 7 * DAY,
        40 => 30 * DAY,
        80 => 365 * DAY,
        _ => DAY,
    }
}

/// `n` of Back In Time's `unit`, as a duration that reads the same ("2years", not
/// "1year 11months 29days…").
fn duration(n: u64, unit: u64) -> HumanDuration {
    let word = match unit {
        10 => "h",
        30 => "weeks",
        40 => "months",
        80 => "years",
        _ => "days",
    };
    format!("{n}{word}")
        .parse()
        .unwrap_or_else(|_| HumanDuration::from_secs(n * unit_secs(unit)))
}

fn retention(p: &mut Profile, k: &Keys<'_>) {
    let r = &mut p.retention;
    let smart = k.bool("snapshots.smart_remove", false);
    let old = k.bool("snapshots.remove_old_snapshots.enabled", false);
    let max_age = old.then(|| {
        duration(
            k.num("snapshots.remove_old_snapshots.value", 10),
            k.num("snapshots.remove_old_snapshots.unit", 80),
        )
    });
    r.keep_named = k.bool("snapshots.dont_remove_named_snapshots", true);
    r.min_free_space = k.bool("snapshots.min_free_space.enabled", false).then(|| {
        let v = k.num("snapshots.min_free_space.value", 1);
        ByteSize(if k.num("snapshots.min_free_space.unit", 20) == 10 {
            v << 20
        } else {
            v << 30
        })
    });
    r.min_free_inodes_percent = k
        .bool("snapshots.min_free_inodes.enabled", false)
        .then(|| k.num("snapshots.min_free_inodes.value", 2).min(100) as u8);
    if smart {
        // Keep everything from the last N days, one a day for N days, one a week for N
        // weeks, one a month for N months: Flux's buckets mean the same.
        r.enabled = true;
        r.keep_last = 1;
        r.keep_within = Some(duration(k.num("snapshots.smart_remove.keep_all", 2), 20));
        r.keep_hourly = 0;
        r.keep_daily = k.num("snapshots.smart_remove.keep_one_per_day", 7) as u32;
        r.keep_weekly = k.num("snapshots.smart_remove.keep_one_per_week", 4) as u32;
        r.keep_monthly = k.num("snapshots.smart_remove.keep_one_per_month", 24) as u32;
        r.keep_yearly = 0;
        r.max_age = max_age;
    } else if let Some(age) = max_age {
        // Only "remove snapshots older than": keep everything younger.
        r.enabled = true;
        r.keep_last = 1;
        r.keep_within = Some(age);
        r.keep_hourly = 0;
        r.keep_daily = 0;
        r.keep_weekly = 0;
        r.keep_monthly = 0;
        r.keep_yearly = 0;
        r.max_age = Some(age);
    } else {
        r.enabled = r.min_free_space.is_some() || r.min_free_inodes_percent.is_some();
    }
}

/// An rsync exclude pattern as a Flux (gitignore) one, or `None` for an empty one.
///
/// rsync matches a pattern with a slash in the middle at any depth; gitignore anchors it
/// at the root, so it gets `**/`. A leading `#` or `!` means something else in gitignore
/// and is escaped; rsync's `***` is `**`.
pub fn pattern(rsync: &str) -> Option<String> {
    let p = rsync.trim();
    if p.is_empty() {
        return None;
    }
    let mut out = p.replace("***", "**");
    let inner = out.trim_end_matches('/');
    if inner.contains('/')
        && !out.starts_with('/')
        && !out.starts_with("~/")
        && !out.starts_with("**/")
    {
        out = format!("**/{out}");
    }
    if out.starts_with('#') || out.starts_with('!') {
        out.insert(0, '\\');
    }
    Some(out)
}

/// Split a command line as a shell would: whitespace separates, quotes group.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut any = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of a real Back In Time 1.5 config (two profiles).
    const CONFIG_V6: &str = "\
config.version=6
profile1.schedule.custom_time=8,12,18,23
profile1.schedule.mode=20
profile1.schedule.time=700
profile1.schedule.weekday=7
profile1.snapshots.backup_on_restore.enabled=true
profile1.snapshots.continue_on_errors=true
profile1.snapshots.dont_remove_named_snapshots=true
profile1.snapshots.exclude.1.value=target/
profile1.snapshots.exclude.2.value=lost+found/*
profile1.snapshots.exclude.3.value=~$*
profile1.snapshots.exclude.4.value=~/.local/share/flatpak
profile1.snapshots.exclude.5.value=#*#
profile1.snapshots.exclude.6.value=.local/share/[Tt]rash*
profile1.snapshots.exclude.7.value=.cache/*
profile1.snapshots.exclude.bysize.enabled=false
profile1.snapshots.exclude.bysize.value=500
profile1.snapshots.exclude.size=7
profile1.snapshots.include.1.type=0
profile1.snapshots.include.1.value=/home/me/Documents
profile1.snapshots.include.2.type=1
profile1.snapshots.include.2.value=/home/me/.bashrc
profile1.snapshots.include.size=2
profile1.snapshots.local_encfs.path=/mnt/external_backup
profile1.snapshots.min_free_space.enabled=true
profile1.snapshots.min_free_space.unit=20
profile1.snapshots.min_free_space.value=50
profile1.snapshots.mode=local
profile1.snapshots.no_on_battery=false
profile1.snapshots.notify.enabled=false
profile1.snapshots.one_file_system=false
profile1.snapshots.path=/mnt/external_backup
profile1.snapshots.path.host=NYX
profile1.snapshots.path.profile=1
profile1.snapshots.path.user=me
profile1.snapshots.preserve_acl=false
profile1.snapshots.preserve_xattr=false
profile1.snapshots.remove_old_snapshots.enabled=true
profile1.snapshots.remove_old_snapshots.unit=80
profile1.snapshots.remove_old_snapshots.value=2
profile1.snapshots.smart_remove=true
profile1.snapshots.smart_remove.keep_all=2
profile1.snapshots.smart_remove.keep_one_per_day=14
profile1.snapshots.smart_remove.keep_one_per_month=12
profile1.snapshots.smart_remove.keep_one_per_week=8
profile1.snapshots.use_checksum=false
profile2.name=test
profile2.schedule.mode=0
profile2.snapshots.exclude.size=0
profile2.snapshots.include.1.type=0
profile2.snapshots.include.1.value=/home/me/Music
profile2.snapshots.include.size=1
profile2.snapshots.mode=ssh_encfs
profile2.snapshots.ssh.host=nas
profile2.snapshots.ssh.path=/srv/bit
profile2.snapshots.ssh.port=2222
profile2.snapshots.ssh.user=me
profile2.snapshots.smart_remove=false
profile2.snapshots.remove_old_snapshots.enabled=false
profile2.snapshots.exclude.bysize.enabled=true
profile2.snapshots.exclude.bysize.value=500
profiles=1:2
profiles.version=1
";

    fn base() -> Profile {
        Profile::new(
            "New",
            Target::Local {
                path: PathBuf::new(),
                fs_uuid: None,
            },
            vec![PathBuf::from("/home/me")],
        )
    }

    fn no_scan(_: &Path) -> (usize, Option<String>) {
        (3, Some("20261101-070001-123".into()))
    }

    #[test]
    fn converts_a_local_profile() {
        let v = parse(CONFIG_V6, &base(), &no_scan);
        assert_eq!(v.len(), 2);
        let b = &v[0];
        assert_eq!(
            (b.number, b.name.as_str(), b.mode.as_str()),
            (1, "Main profile", "local")
        );
        let p = &b.profile;
        assert_eq!(p.engine, Engine::Rsync);
        assert_eq!(
            p.sources.paths,
            [
                PathBuf::from("/home/me/Documents"),
                PathBuf::from("/home/me/.bashrc")
            ]
        );
        assert_eq!(
            p.sources.exclude,
            [
                "target/",
                "**/lost+found/*",
                "~$*",
                "~/.local/share/flatpak",
                "\\#*#",
                "**/.local/share/[Tt]rash*",
                "**/.cache/*",
            ]
        );
        assert_eq!(p.sources.exclude_larger_than, None);
        assert!(
            matches!(&p.target, Target::Local { path, .. } if path == Path::new("/mnt/external_backup"))
        );
        assert_eq!(
            b.snapshots_dir.as_deref(),
            Some(Path::new("/mnt/external_backup/backintime/NYX/me/1"))
        );
        assert_eq!(
            (b.snapshot_count, b.latest.as_deref()),
            (3, Some("20261101-070001-123"))
        );
        assert_eq!(p.schedule.trigger, Trigger::Calendar);
        assert_eq!(p.schedule.on_calendar.as_deref(), Some("*-*-* 07:00:00"));
        let r = &p.retention;
        assert!(r.enabled && r.keep_named);
        assert_eq!(r.keep_within, Some(HumanDuration::from_secs(2 * DAY)));
        assert_eq!(
            (r.keep_daily, r.keep_weekly, r.keep_monthly, r.keep_yearly),
            (14, 8, 12, 0)
        );
        assert_eq!(r.max_age.unwrap().to_string(), "2years");
        assert_eq!(r.min_free_space, Some(ByteSize(50 << 30)));
        assert!(!p.options.preserve_acl && !p.options.preserve_xattr && !p.options.notify);
        // The converted profile is one Flux accepts as is.
        let errors: Vec<_> = p
            .validate()
            .into_iter()
            .filter(|i| i.severity == flux_core::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(b.notes.iter().any(|n| n.contains("written differently")));
    }

    #[test]
    fn converts_an_encrypted_ssh_profile() {
        let v = parse(CONFIG_V6, &base(), &no_scan);
        let b = &v[1];
        assert_eq!((b.name.as_str(), b.mode.as_str()), ("test", "ssh_encfs"));
        let p = &b.profile;
        assert_eq!(p.engine, Engine::Vault);
        assert!(b.notes.iter().any(|n| n.contains("EncFS")));
        match &p.target {
            Target::Ssh {
                host,
                user,
                port,
                path,
                ..
            } => {
                assert_eq!(
                    (host.as_str(), user.as_deref(), *port),
                    ("nas", Some("me"), Some(2222))
                );
                assert_eq!(path, Path::new("/srv/bit"));
            }
            t => panic!("{t:?}"),
        }
        assert!(b.snapshots_dir.is_none());
        assert_eq!(p.schedule.trigger, Trigger::Manual);
        assert!(
            !p.retention.enabled,
            "keeps everything, like Back In Time did"
        );
        assert_eq!(p.sources.exclude_larger_than, Some(ByteSize(500 << 20)));
    }

    #[test]
    fn extra_rsync_options_carry_over() {
        let with = |mode: &str, opts: &str| {
            let text = format!(
                "profiles=1\nprofile1.snapshots.mode={mode}\nprofile1.snapshots.include.1.value=/a\nprofile1.snapshots.include.size=1\nprofile1.snapshots.rsync_options.enabled=true\nprofile1.snapshots.rsync_options.value={opts}"
            );
            parse(&text, &base(), &no_scan).remove(0)
        };
        let b = with("local", "--compress --chmod='u+rw' --link-dest=/x --sparse");
        assert_eq!(
            b.profile.rsync.as_ref().map(|r| r.extra_args.clone()),
            Some(vec![
                "--compress".into(),
                "--chmod=u+rw".into(),
                "--sparse".into()
            ])
        );
        assert!(
            b.notes.iter().any(|n| n.contains("--link-dest=/x")),
            "{:?}",
            b.notes
        );
        assert!(
            b.profile
                .validate()
                .iter()
                .all(|i| !i.field.starts_with("rsync"))
        );
        let b = with("local_encfs", "--compress");
        assert!(b.profile.rsync.is_none());
        assert!(b.notes.iter().any(|n| n.contains("doesn't use rsync")));
        assert_eq!(
            split_args(r#"-a "two words" 'x y' a\ b"#),
            ["-a", "two words", "x y", "a b"]
        );
    }

    #[test]
    fn schedules() {
        let with = |extra: &str| {
            let text = format!(
                "profiles=1\nprofile1.snapshots.include.1.value=/a\nprofile1.snapshots.include.size=1\n{extra}"
            );
            parse(&text, &base(), &no_scan).remove(0).profile.schedule
        };
        let s = with(
            "profile1.schedule.mode=30\nprofile1.schedule.weekday=1\nprofile1.schedule.time=2130",
        );
        assert_eq!(s.on_calendar.as_deref(), Some("Mon *-*-* 21:30:00"));
        let s = with("profile1.schedule.mode=19\nprofile1.schedule.custom_time=8,12,18");
        assert_eq!(s.on_calendar.as_deref(), Some("*-*-* 8,12,18:00:00"));
        let s = with("profile1.schedule.mode=14");
        assert_eq!(
            (s.trigger, s.every),
            (Trigger::Interval, Some(HumanDuration::from_secs(4 * 3600)))
        );
        let s = with(
            "profile1.schedule.mode=25\nprofile1.schedule.repeatedly.period=3\nprofile1.schedule.repeatedly.unit=20",
        );
        assert_eq!(s.every, Some(HumanDuration::from_secs(3 * DAY)));
        let s = with("profile1.schedule.mode=10");
        assert_eq!(s.on_calendar.as_deref(), Some("hourly"));
    }

    #[test]
    fn missing_config_is_no_profiles() {
        let v = read(Path::new("/nonexistent/backintime/config"), &base()).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn snapshot_names() {
        assert!(is_snapshot_name("20261101-070001-123"));
        assert!(!is_snapshot_name("last_snapshot"));
        assert!(!is_snapshot_name("20261101-0700"));
    }
}
