//! The backup drive's own health report, read from udisks without an administrator
//! password: SMART on ATA disks (says it's failing; bad sectors), and an NVMe drive's
//! critical warnings (spare space running out, no longer reliable, too hot). Checked when a
//! backup to the drive runs; what was seen is kept, so the Overview can say it and a
//! notification says it once. Most USB sticks and many USB enclosures report nothing:
//! then Flux says it can't tell.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use zbus::OwnedVariant as OwnedValue;

use flux_core::{Paths, Profile, Target};

const UDISKS: &str = "org.freedesktop.UDisks2";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// What a drive says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Nothing (no SMART through this enclosure, a USB stick, not read yet).
    Nothing,
    Ata {
        failing: bool,
        bad_sectors: i64,
    },
    /// NVMe critical warnings: `spare`, `temperature`, `degraded`, `readonly`,
    /// `volatile_mem`, `pmr_readonly`.
    Nvme {
        warnings: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// The drive's model, for words ("Samsung T7"); may be empty.
    pub model: String,
    /// udisks' id for the drive, to notice when another one is in its place.
    pub id: String,
    pub report: Report,
}

/// What was last seen of one place's drive (kept in the state folder).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Known {
    pub checked: Option<Timestamp>,
    pub model: String,
    pub id: String,
    /// It reports its health at all.
    pub reports: bool,
    pub failing: bool,
    pub bad_sectors: Option<i64>,
    /// The bad sectors when Flux first looked at this drive.
    pub bad_baseline: Option<i64>,
    /// When the bad sectors last grew.
    pub grew_at: Option<Timestamp>,
    pub nvme_warnings: Vec<String>,
    /// What was last notified, so it's said once.
    pub told: Option<String>,
}

/// What's worth saying, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// It's failing (not only showing signs).
    pub danger: bool,
    pub text: String,
    pub hint: String,
}

/// How long bad sectors that grew are mentioned.
const GREW_FOR: SignedDuration = SignedDuration::from_hours(30 * 24);

impl Known {
    /// Taken in: `r`, seen at `now`, after what was known before (`self`).
    pub fn update(self, r: &Reading, now: Timestamp) -> Known {
        let same = self.id == r.id && !self.id.is_empty();
        let prev = if same { self } else { Known::default() };
        let mut k = Known {
            checked: Some(now),
            model: r.model.clone(),
            id: r.id.clone(),
            told: prev.told.clone(),
            ..Known::default()
        };
        match &r.report {
            Report::Nothing => {}
            Report::Ata {
                failing,
                bad_sectors,
            } => {
                k.reports = true;
                k.failing = *failing;
                k.bad_sectors = Some(*bad_sectors);
                k.bad_baseline = Some(prev.bad_baseline.unwrap_or(*bad_sectors));
                k.grew_at = if prev.bad_sectors.is_some_and(|b| *bad_sectors > b) {
                    Some(now)
                } else {
                    prev.grew_at
                };
            }
            Report::Nvme { warnings } => {
                k.reports = true;
                k.nvme_warnings = warnings.clone();
            }
        }
        k
    }

    /// What to say about the drive at `now`, if anything.
    pub fn said(&self, now: Timestamp) -> Option<Said> {
        let drive = if self.model.trim().is_empty() {
            "The backup drive".to_owned()
        } else {
            format!("The backup drive ({})", self.model.trim())
        };
        let replace = "Copy the backups to a new drive soon, and stop using this one.".to_owned();
        if self.failing {
            return Some(Said {
                danger: true,
                text: format!("{drive} says it's failing."),
                hint: replace,
            });
        }
        let w = |k: &str| self.nvme_warnings.iter().any(|x| x == k);
        if w("spare") {
            return Some(Said {
                danger: true,
                text: format!("{drive} is running out of spare space for worn-out cells."),
                hint: replace,
            });
        }
        if w("degraded") || w("readonly") || w("volatile_mem") || w("pmr_readonly") {
            return Some(Said {
                danger: true,
                text: format!("{drive} says it's no longer reliable."),
                hint: replace,
            });
        }
        if let (Some(bad), Some(base), Some(at)) =
            (self.bad_sectors, self.bad_baseline, self.grew_at)
            && bad > base
            && now.duration_since(at) < GREW_FOR
        {
            return Some(Said {
                danger: false,
                text: format!(
                    "{drive} has {bad} bad sectors, {} more than when Flux first looked.",
                    bad - base
                ),
                hint: "Drives often fail soon after this starts: copy the backups to a new drive soon.".into(),
            });
        }
        if w("temperature") {
            return Some(Said {
                danger: false,
                text: format!("{drive} says it's running too hot."),
                hint: "Give it some air, or move it away from heat.".into(),
            });
        }
        None
    }
}

/// `<id>` for a profile's main place, `<id>+<name>` for a destination.
fn state_path(paths: &Paths, key: &str) -> PathBuf {
    paths
        .state_dir
        .join("drive-health")
        .join(format!("{key}.json"))
}

pub fn load(paths: &Paths, key: &str) -> Option<Known> {
    serde_json::from_slice(&std::fs::read(state_path(paths, key)).ok()?).ok()
}

fn save(paths: &Paths, key: &str, k: &Known) {
    let p = state_path(paths, key);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(k) {
        let _ = flux_core::store::write_atomic(&p, &bytes, 0o600);
    }
}

/// The places a profile keeps backups on local drives: its own (`<id>`) and its
/// destinations' (`<id>+<name>`).
pub fn places(id: &str, p: &Profile) -> Vec<(String, PathBuf)> {
    let main = std::iter::once((id.to_owned(), &p.target));
    let dests = p
        .destinations
        .iter()
        .map(|d| (format!("{id}+{}", d.name), &d.target));
    main.chain(dests)
        .filter_map(|(k, t)| match t {
            Target::Local { path, .. } if !path.as_os_str().is_empty() => Some((k, path.clone())),
            _ => None,
        })
        .collect()
}

/// udisks escapes a device name for an object path: anything but letters and digits as
/// `_xx` (`dm-0` → `dm_2d0`).
fn object_name(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("_{b:02x}")
            }
        })
        .collect()
}

async fn get_all(
    conn: &zbus::Connection,
    path: &str,
    iface: &str,
) -> Option<HashMap<String, OwnedValue>> {
    let m = conn
        .call_method(Some(UDISKS), path, Some(PROPERTIES), "GetAll", &(iface,))
        .await
        .ok()?;
    m.body().deserialize().ok()
}

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    T::try_from(props.get(key)?.try_clone().ok()?).ok()
}

/// What whole disk `disk` (`sdb`, `nvme0n1`) says about itself, asked on `conn`. `None`:
/// udisks doesn't know the disk.
pub async fn read_on(conn: &zbus::Connection, disk: &str) -> Option<Reading> {
    let block = format!(
        "/org/freedesktop/UDisks2/block_devices/{}",
        object_name(disk)
    );
    let b = get_all(conn, &block, "org.freedesktop.UDisks2.Block").await?;
    let drive: zbus::OwnedObjectPath = get(&b, "Drive")?;
    if drive.as_str() == "/" {
        return None;
    }
    let d = get_all(conn, drive.as_str(), "org.freedesktop.UDisks2.Drive")
        .await
        .unwrap_or_default();
    let text = |k: &str| get::<String>(&d, k).unwrap_or_default();
    let model = [text("Vendor"), text("Model")]
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let id = text("Id");
    let report = if let Some(a) =
        get_all(conn, drive.as_str(), "org.freedesktop.UDisks2.Drive.Ata").await
        && get::<bool>(&a, "SmartSupported").unwrap_or(false)
        && get::<u64>(&a, "SmartUpdated").unwrap_or(0) > 0
    {
        Report::Ata {
            failing: get(&a, "SmartFailing").unwrap_or(false),
            bad_sectors: get(&a, "SmartNumBadSectors").unwrap_or(0),
        }
    } else if let Some(n) = get_all(
        conn,
        drive.as_str(),
        "org.freedesktop.UDisks2.NVMe.Controller",
    )
    .await
        && get::<u64>(&n, "SmartUpdated").unwrap_or(0) > 0
    {
        Report::Nvme {
            warnings: get(&n, "SmartCriticalWarning").unwrap_or_default(),
        }
    } else {
        Report::Nothing
    };
    Some(Reading { model, id, report })
}

/// What the drive holding `path` says about itself (on the system bus). `None`: not on a
/// drive udisks knows (unplugged, the system disk under LVM, a network share).
pub fn read(path: &Path) -> Option<Reading> {
    let disk = crate::release::disk_of(path)?;
    zbus::block_on(async {
        let conn = zbus::Connection::system().await.ok()?;
        read_on(&conn, &disk).await
    })
}

/// After a backup: read each local place's drive, keep what was seen, and return what's
/// newly worth telling (`(key, said)`), each once until it changes.
pub fn check_places(paths: &Paths, id: &str, p: &Profile, now: Timestamp) -> Vec<(String, Said)> {
    let mut out = Vec::new();
    for (key, path) in places(id, p) {
        let Some(r) = read(&path) else { continue };
        let mut k = load(paths, &key).unwrap_or_default().update(&r, now);
        match k.said(now) {
            Some(s) if k.told.as_deref() != Some(s.text.as_str()) => {
                k.told = Some(s.text.clone());
                out.push((key.clone(), s));
            }
            Some(_) => {}
            None => k.told = None,
        }
        save(paths, &key, &k);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    fn ata(failing: bool, bad: i64) -> Reading {
        Reading {
            model: "WD Elements".into(),
            id: "WD-1".into(),
            report: Report::Ata {
                failing,
                bad_sectors: bad,
            },
        }
    }

    #[test]
    fn bad_sectors_count_once_they_grow() {
        let t0 = Timestamp::from_second(1_800_000_000).unwrap();
        let day = SignedDuration::from_hours(24);
        // Five from the start: an old drive's, not news.
        let k = Known::default().update(&ata(false, 5), t0);
        assert_eq!(k.said(t0), None);
        let k = k.update(&ata(false, 5), t0 + day);
        assert_eq!(k.said(t0 + day), None);
        // Seven: two more.
        let k = k.update(&ata(false, 7), t0 + day * 2);
        let s = k.said(t0 + day * 2).unwrap();
        assert!(
            !s.danger && s.text.contains("7 bad sectors, 2 more"),
            "{}",
            s.text
        );
        // Still said a week on; not after a month without growing.
        let k = k.update(&ata(false, 7), t0 + day * 9);
        assert!(k.said(t0 + day * 9).is_some());
        assert_eq!(k.said(t0 + day * 40), None);
        // Failing outranks it.
        let k = k.update(&ata(true, 7), t0 + day * 10);
        assert!(k.said(t0 + day * 10).unwrap().danger);
        // Another drive in its place starts afresh.
        let other = Reading {
            id: "WD-2".into(),
            ..ata(false, 9)
        };
        let k = k.update(&other, t0 + day * 11);
        assert_eq!(k.said(t0 + day * 11), None);
        assert_eq!(k.bad_baseline, Some(9));
    }

    #[test]
    fn nvme_warnings_and_drives_that_say_nothing() {
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let nvme = |w: &[&str]| Reading {
            model: String::new(),
            id: "N".into(),
            report: Report::Nvme {
                warnings: w.iter().map(|s| s.to_string()).collect(),
            },
        };
        let s = Known::default()
            .update(&nvme(&["spare"]), now)
            .said(now)
            .unwrap();
        assert!(
            s.danger
                && s.text
                    .starts_with("The backup drive is running out of spare")
        );
        let s = Known::default()
            .update(&nvme(&["temperature"]), now)
            .said(now)
            .unwrap();
        assert!(!s.danger);
        assert_eq!(Known::default().update(&nvme(&[]), now).said(now), None);
        let quiet = Reading {
            model: "SanDisk".into(),
            id: "S".into(),
            report: Report::Nothing,
        };
        let k = Known::default().update(&quiet, now);
        assert!(!k.reports && k.said(now).is_none());
    }

    /// This computer's own disk, through the real udisks (read only): run with --ignored.
    #[test]
    #[ignore]
    fn reads_this_computers_disk() {
        let r = read(Path::new("/"));
        eprintln!("{r:?}");
        assert!(r.is_some());
    }

    #[test]
    fn udisks_names() {
        assert_eq!(object_name("sdb"), "sdb");
        assert_eq!(object_name("nvme0n1"), "nvme0n1");
        assert_eq!(object_name("dm-0"), "dm_2d0");
    }

    /// A stand-in udisks on a private bus: block `sdb` on drive `WD`, whose SMART says
    /// it's failing with 3 bad sectors; block `sdc` on a drive with no SMART.
    #[test]
    fn read_from_udisks() {
        let Ok(mut child) = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
        else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let server = zbus::block_on(async {
            let c = zbus::conn::Builder::address(address.as_str())
                .build()
                .await
                .unwrap();
            c.request_name(UDISKS).await.unwrap();
            c
        });
        let serve = server.clone();
        std::thread::spawn(move || {
            zbus::block_on(async move {
                let mut incoming = zbus::MessageStream::from(&serve);
                while let Some(Ok(msg)) = incoming.next().await {
                    let h = msg.header();
                    if h.message_type() != zbus::message::Type::MethodCall
                        || h.member().map(|m| m.as_str()) != Some("GetAll")
                    {
                        continue;
                    }
                    let path = h.path().map(|p| p.as_str().to_owned()).unwrap_or_default();
                    let iface: String = msg.body().deserialize().unwrap_or_default();
                    let o = |p: &str| {
                        zbus::Variant::from(zbus::ObjectPath::try_from(p.to_owned()).unwrap())
                    };
                    let mut props: HashMap<&str, zbus::Variant<'_>> = HashMap::new();
                    match (path.as_str(), iface.as_str()) {
                        (
                            "/org/freedesktop/UDisks2/block_devices/sdb",
                            "org.freedesktop.UDisks2.Block",
                        ) => {
                            props.insert("Drive", o("/org/freedesktop/UDisks2/drives/WD"));
                        }
                        (
                            "/org/freedesktop/UDisks2/block_devices/sdc",
                            "org.freedesktop.UDisks2.Block",
                        ) => {
                            props.insert("Drive", o("/org/freedesktop/UDisks2/drives/Stick"));
                        }
                        (_, "org.freedesktop.UDisks2.Drive") => {
                            props.insert("Vendor", "WD".into());
                            props.insert("Model", "Elements".into());
                            props.insert("Id", "WD-Elements-123".into());
                        }
                        (
                            "/org/freedesktop/UDisks2/drives/WD",
                            "org.freedesktop.UDisks2.Drive.Ata",
                        ) => {
                            props.insert("SmartSupported", true.into());
                            props.insert("SmartUpdated", 1_800_000_000u64.into());
                            props.insert("SmartFailing", true.into());
                            props.insert("SmartNumBadSectors", 3i64.into());
                        }
                        _ => {
                            let _ = serve
                                .reply_error(
                                    &h,
                                    "org.freedesktop.DBus.Error.UnknownInterface",
                                    &"no such interface",
                                )
                                .await;
                            continue;
                        }
                    }
                    let _ = serve.reply(&h, &props).await;
                }
            });
        });
        let (wd, stick, none) = zbus::block_on(async {
            let c = zbus::conn::Builder::address(address.as_str())
                .build()
                .await
                .unwrap();
            (
                read_on(&c, "sdb").await,
                read_on(&c, "sdc").await,
                read_on(&c, "sdz").await,
            )
        });
        let _ = child.kill();
        let wd = wd.unwrap();
        assert_eq!(wd.model, "WD Elements");
        assert_eq!(
            wd.report,
            Report::Ata {
                failing: true,
                bad_sectors: 3
            }
        );
        assert_eq!(stick.unwrap().report, Report::Nothing);
        assert_eq!(none, None);
    }
}
