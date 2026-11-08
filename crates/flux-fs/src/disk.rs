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
