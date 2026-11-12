//! Spinning disks, and how many large files to move at once on them.
//!
//! A hard disk has one head: copying (or reading) sixteen large files at once makes it seek
//! between them, and the whole backup gets slower, not faster. Backups often go to a USB
//! hard disk, so when a source or the target is on one, large files are moved
//! [`ROTATIONAL_STREAMS`] at a time. Small files, links and metadata stay fully parallel.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

/// Files at least this big count as large (they stream for a while).
pub const LARGE_FILE: u64 = 4 << 20;
/// Large files moved at once when a spinning disk is involved.
pub const ROTATIONAL_STREAMS: usize = 2;

/// Is `path` on a spinning disk? `None` when it can't be told (a network filesystem,
/// no sysfs).
pub fn is_rotational(path: &Path) -> Option<bool> {
    rotational_in(path, Path::new("/sys"), Path::new("/proc/self/mountinfo"))
}

fn rotational_in(path: &Path, sys: &Path, mountinfo: &Path) -> Option<bool> {
    let path = fs::canonicalize(path).ok()?;
    let dev = fs::metadata(&path).ok()?.dev();
    if let Some(r) = queue_rotational(sys, rustix::fs::major(dev), rustix::fs::minor(dev)) {
        return Some(r);
    }
    // Btrfs and others report a virtual device number: ask the device the mount comes from.
    let source = mount_source(&path, &fs::read_to_string(mountinfo).ok()?)?;
    let rdev = fs::metadata(source).ok()?.rdev();
    queue_rotational(sys, rustix::fs::major(rdev), rustix::fs::minor(rdev))
}

/// Something about the drive a path is on that makes backups to it slower than they could
/// be, in words, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveNote {
    /// `trim` or `usb_speed`.
    pub kind: &'static str,
    pub text: String,
    pub hint: String,
}

/// What [`DriveNote`]s the drive under `path` calls for: an SSD that TRIM doesn't reach
/// (through its USB adapter, or through LUKS), or a USB 3 drive running at USB 2 speed.
/// Nothing when it can't be told (a network filesystem, no sysfs).
pub fn drive_notes(path: &Path) -> Vec<DriveNote> {
    notes_in(path, Path::new("/sys"), Path::new("/proc/self/mountinfo"))
}

fn notes_in(path: &Path, sys: &Path, mountinfo: &Path) -> Vec<DriveNote> {
    mounted_block(path, sys, mountinfo).map_or_else(Vec::new, |top| notes_of(&top))
}

/// The notes for the block device at `top` (its sysfs folder) and the disks under it.
fn notes_of(top: &Path) -> Vec<DriveNote> {
    let mut out = Vec::new();
    for disk in disks_under(top, 0) {
        if let Some(n) = trim_note(top, &disk) {
            out.push(n);
        }
        if let Some(n) = usb_note(&disk) {
            out.push(n);
        }
    }
    out.dedup();
    out
}

/// The sysfs folder of the block device `path`'s filesystem is mounted from.
fn mounted_block(path: &Path, sys: &Path, mountinfo: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    let at = |dev: u64| {
        fs::canonicalize(sys.join(format!(
            "dev/block/{}:{}",
            rustix::fs::major(dev),
            rustix::fs::minor(dev)
        )))
        .ok()
    };
    at(fs::metadata(&path).ok()?.dev()).or_else(|| {
        // Btrfs and others report a virtual device number: the mount's source.
        let source = mount_source(&path, &fs::read_to_string(mountinfo).ok()?)?;
        at(fs::metadata(source).ok()?.rdev())
    })
}

/// The disks under a block device: through device-mapper (LUKS, LVM) to what it is made
/// of, and from a partition to its disk.
fn disks_under(dev: &Path, depth: u8) -> Vec<PathBuf> {
    let slaves: Vec<PathBuf> = fs::read_dir(dev.join("slaves"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| fs::canonicalize(e.path()).ok())
                .collect()
        })
        .unwrap_or_default();
    if !slaves.is_empty() && depth < 8 {
        return slaves
            .iter()
            .flat_map(|s| disks_under(s, depth + 1))
            .collect();
    }
    if dev.join("partition").is_file() {
        return dev.parent().map(Path::to_path_buf).into_iter().collect();
    }
    vec![dev.to_path_buf()]
}

/// A number in a sysfs file of a device's queue (a partition's is its disk's).
fn queue_value(dev: &Path, name: &str) -> Option<u64> {
    let queue = if dev.join("queue").is_dir() {
        dev.join("queue")
    } else {
        dev.join("../queue")
    };
    fs::read_to_string(queue.join(name))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// An SSD that TRIM doesn't reach can't tell which of its space is free: long writes to
/// it slow down as it fills, more with every backup deleted from it. (A USB SSD here wrote
/// 860 MiB/s at first and 250–530 after 170 GiB.)
fn trim_note(top: &Path, disk: &Path) -> Option<DriveNote> {
    if queue_value(disk, "rotational")? != 0 {
        return None;
    }
    let on_disk = queue_value(disk, "discard_max_bytes")? > 0;
    let through = queue_value(top, "discard_max_bytes").unwrap_or(0) > 0;
    let usb = disk.to_string_lossy().contains("/usb");
    if !on_disk {
        let mode = fs::read_dir(disk.join("device/scsi_disk"))
            .ok()
            .and_then(|mut rd| rd.next())
            .and_then(|e| e.ok())
            .filter(|e| e.path().join("provisioning_mode").is_file())
            // The same file, by the short name people know: /sys/block/sda/….
            .map(|e| {
                Path::new("/sys/block")
                    .join(disk.file_name().unwrap_or_default())
                    .join("device/scsi_disk")
                    .join(e.file_name())
                    .join("provisioning_mode")
            });
        return Some(DriveNote {
            kind: "trim",
            text: if usb {
                "TRIM doesn't reach this SSD through its USB adapter: the drive can't tell which of its space is free, so long backups to it slow down as it fills."
            } else {
                "TRIM doesn't reach this SSD: it can't tell which of its space is free, so long backups to it slow down as it fills."
            }
            .into(),
            hint: match mode {
                Some(m) if usb => format!(
                    "Many USB adapters pass TRIM on once told to. As root: echo unmap > {} (a udev rule keeps it), then fstrim the drive. Some adapters can't.",
                    m.display()
                ),
                _ => "If the drive supports TRIM another way, run fstrim on it now and then.".into(),
            },
        });
    }
    (!through).then(|| DriveNote {
        kind: "trim",
        text: "TRIM doesn't pass through this drive's encryption (LUKS): the SSD can't tell which of its space is free, so long backups to it slow down as it fills.".into(),
        hint: "Add discard to the drive's line in /etc/crypttab (or open it with cryptsetup --allow-discards), then fstrim it.".into(),
    })
}

/// A USB 3 drive linked at USB 2 speed: a USB 2 port, hub or cable on the way.
fn usb_note(disk: &Path) -> Option<DriveNote> {
    // The USB device: the nearest folder above the disk with a speed and a version, below
    // the bus's root hub (`usbN`).
    let device = disk.ancestors().find(|d| {
        d.join("speed").is_file()
            && d.join("version").is_file()
            && !d
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("usb"))
    })?;
    let read = |f: &str| fs::read_to_string(device.join(f)).ok();
    let speed: f64 = read("speed")?.trim().parse().ok()?;
    let version: f64 = read("version")?.trim().parse().ok()?;
    (version >= 3.0 && speed <= 480.0).then(|| DriveNote {
        kind: "usb_speed",
        text: "The backup drive is running at USB 2 speed (480 Mbit/s: about 40 MB/s at most), though it's a USB 3 drive.".into(),
        hint: "Plug it straight into a USB 3 port (often blue, or marked SS, 5, 10 or 20), with the cable it came with.".into(),
    })
}

/// `queue/rotational` of a block device, or of the disk a partition is on.
fn queue_rotational(sys: &Path, major: u32, minor: u32) -> Option<bool> {
    let dev = sys.join(format!("dev/block/{major}:{minor}"));
    [
        dev.join("queue/rotational"),
        dev.join("../queue/rotational"),
    ]
    .iter()
    .find_map(|q| fs::read_to_string(q).ok())
    .map(|s| s.trim() == "1")
}

/// The device a path's filesystem is mounted from (`/dev/…`), from `/proc/self/mountinfo`:
/// the longest mount point that contains `path`.
fn mount_source(path: &Path, mountinfo: &str) -> Option<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|l| {
            let fields: Vec<&str> = l.split(' ').collect();
            let mount_point = PathBuf::from(unescape(fields.get(4)?));
            let dash = fields.iter().position(|f| *f == "-")?;
            let source = unescape(fields.get(dash + 2)?);
            (path.starts_with(&mount_point) && source.starts_with("/dev/"))
                .then_some((mount_point, source))
        })
        .max_by_key(|(m, _)| m.as_os_str().len())
        .map(|(_, s)| PathBuf::from(s))
}

/// mountinfo escapes spaces and such as `\040`.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8)
        {
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lets at most `limit` large files through at once (no limit: everything passes).
#[derive(Debug)]
pub struct IoGate {
    limit: Option<usize>,
    busy: Mutex<usize>,
    freed: Condvar,
}

impl IoGate {
    pub fn unlimited() -> Self {
        Self::new(None)
    }

    fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            busy: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Limited to [`ROTATIONAL_STREAMS`] if any of `paths` is on a spinning disk.
    pub fn for_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Self {
        let spinning = paths.into_iter().any(|p| is_rotational(p) == Some(true));
        Self::new(spinning.then_some(ROTATIONAL_STREAMS))
    }

    /// True when large files are being limited (for a log line).
    pub fn is_limited(&self) -> bool {
        self.limit.is_some()
    }

    /// Wait for a turn to move a file of `size` bytes; the turn ends when the guard drops.
    /// Small files never wait.
    pub fn enter(&self, size: u64) -> Option<GateGuard<'_>> {
        let limit = self.limit?;
        if size < LARGE_FILE {
            return None;
        }
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        while *busy >= limit {
            busy = self.freed.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        Some(GateGuard { gate: self })
    }
}

pub struct GateGuard<'a> {
    gate: &'a IoGate,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        let mut busy = self.gate.busy.lock().unwrap_or_else(|e| e.into_inner());
        *busy -= 1;
        self.gate.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {

    /// A fake sysfs: a USB device (`speed`, `version`) with a disk `sda` (its queue's
    /// `rotational` and `discard_max_bytes`), partition `sda1`, and LUKS on it (`dm-1`,
    /// discard as `dm_discard`). Returns the folder of the device a filesystem would be
    /// mounted from (`dm-1`, or `sda1` without LUKS).
    fn fake_sys(
        root: &Path,
        usb: Option<(&str, &str)>,
        rotational: u8,
        discard: u64,
        dm_discard: Option<u64>,
    ) -> PathBuf {
        let w = |p: PathBuf, v: &str| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, v).unwrap();
        };
        let mut base = root.join("devices/pci0000:00/0000:35:00.3");
        if let Some((speed, version)) = usb {
            w(base.join("usb7/speed"), "10000\n");
            w(base.join("usb7/version"), " 3.10\n");
            base = base.join("usb7/7-1");
            w(base.join("speed"), &format!("{speed}\n"));
            w(base.join("version"), &format!(" {version}\n"));
            base = base.join("7-1:1.0/host0/target0:0:0/0:0:0:0");
            w(base.join("scsi_disk/0:0:0:0/provisioning_mode"), "full\n");
        } else {
            base = base.join("nvme/nvme0");
        }
        let disk = base.join("block/sda");
        w(disk.join("queue/rotational"), &format!("{rotational}\n"));
        w(
            disk.join("queue/discard_max_bytes"),
            &format!("{discard}\n"),
        );
        if usb.is_some() {
            std::os::unix::fs::symlink(base.clone(), disk.join("device")).unwrap();
        }
        w(disk.join("sda1/partition"), "1\n");
        let Some(d) = dm_discard else {
            return disk.join("sda1");
        };
        let dm = root.join("devices/virtual/block/dm-1");
        w(dm.join("queue/discard_max_bytes"), &format!("{d}\n"));
        w(dm.join("queue/rotational"), "0\n");
        fs::create_dir_all(dm.join("slaves")).unwrap();
        std::os::unix::fs::symlink(disk.join("sda1"), dm.join("slaves/sda1")).unwrap();
        dm
    }

    fn kinds(notes: &[DriveNote]) -> Vec<&'static str> {
        notes.iter().map(|n| n.kind).collect()
    }

    #[test]
    fn drive_notes_for_trim_and_usb_speed() {
        // The owner's drive: a USB SSD at 10 Gbit/s whose adapter doesn't pass TRIM, under
        // LUKS. One note, about the adapter, saying how to try unmap.
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), Some(("10000", "3.20")), 0, 0, Some(0));
        let notes = notes_of(&top);
        assert_eq!(kinds(&notes), ["trim"], "{notes:?}");
        assert!(notes[0].text.contains("USB adapter"), "{notes:?}");
        assert!(
            notes[0].hint.contains("unmap > ") && notes[0].hint.contains("provisioning_mode"),
            "{notes:?}"
        );

        // The disk takes TRIM; LUKS doesn't pass it on.
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), Some(("10000", "3.20")), 0, 4 << 30, Some(0));
        let notes = notes_of(&top);
        assert_eq!(kinds(&notes), ["trim"]);
        assert!(notes[0].hint.contains("crypttab"), "{notes:?}");

        // TRIM all the way through, with LUKS and without: nothing to say.
        for dm in [Some(4 << 30), None] {
            let t = tempfile::tempdir().unwrap();
            let top = fake_sys(t.path(), Some(("10000", "3.20")), 0, 4 << 30, dm);
            assert!(notes_of(&top).is_empty(), "{dm:?}");
        }

        // A hard disk: TRIM isn't a thing there.
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), Some(("5000", "3.00")), 1, 0, None);
        assert!(notes_of(&top).is_empty());

        // A USB 3 drive on a USB 2 port (or hub, or cable).
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), Some(("480", "3.10")), 1, 0, None);
        assert_eq!(kinds(&notes_of(&top)), ["usb_speed"]);
        // A USB 2 stick at USB 2 speed is as fast as it gets.
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), Some(("480", "2.00")), 1, 0, None);
        assert!(notes_of(&top).is_empty());

        // An internal NVMe drive with TRIM: nothing.
        let t = tempfile::tempdir().unwrap();
        let top = fake_sys(t.path(), None, 0, 2 << 40, None);
        assert!(notes_of(&top).is_empty());
    }

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[test]
    fn mount_source_takes_the_longest_mount_point() {
        let info = "\
46 1 0:37 /root / rw,relatime shared:1 - btrfs /dev/mapper/luks-abc rw
50 46 8:17 / /run/media/me/My\\040Disk rw - ext4 /dev/sdb1 rw
51 46 0:40 / /mnt/nas rw - nfs4 nas:/srv rw";
        let src = |p: &str| mount_source(Path::new(p), info);
        assert_eq!(src("/home/me"), Some("/dev/mapper/luks-abc".into()));
        assert_eq!(src("/run/media/me/My Disk/flux"), Some("/dev/sdb1".into()));
        // A network filesystem has no block device: the root's isn't it either.
        assert_eq!(src("/mnt/nas/x"), Some("/dev/mapper/luks-abc".into()));
    }

    #[test]
    fn reads_rotational_of_a_device_or_its_disk() {
        let sys = tempfile::tempdir().unwrap();
        let disk = sys.path().join("devices/sdb");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("queue/rotational"), "1\n").unwrap();
        fs::create_dir_all(disk.join("sdb1")).unwrap();
        fs::create_dir_all(sys.path().join("dev/block")).unwrap();
        std::os::unix::fs::symlink(&disk, sys.path().join("dev/block/8:16")).unwrap();
        std::os::unix::fs::symlink(disk.join("sdb1"), sys.path().join("dev/block/8:17")).unwrap();
        assert_eq!(queue_rotational(sys.path(), 8, 16), Some(true));
        assert_eq!(
            queue_rotational(sys.path(), 8, 17),
            Some(true),
            "a partition: its disk"
        );
        assert_eq!(queue_rotational(sys.path(), 9, 9), None);
    }

    #[test]
    fn gate_limits_large_files_only() {
        let gate = IoGate::new(Some(2));
        let (now, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let _g = gate.enter(LARGE_FILE);
                    let n = now.fetch_add(1, SeqCst) + 1;
                    peak.fetch_max(n, SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    now.fetch_sub(1, SeqCst);
                });
            }
        });
        assert_eq!(peak.load(SeqCst), 2);
        assert!(
            gate.enter(LARGE_FILE - 1).is_none(),
            "small files never wait"
        );
        assert!(IoGate::unlimited().enter(u64::MAX).is_none());
    }
}
