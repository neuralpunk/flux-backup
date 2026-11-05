//! Facts about the running system: host and user names, privileges, mounts.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Short host name (as `uname -n`, cut at the first dot).
pub fn hostname() -> String {
    let uts = rustix::system::uname();
    let name = uts.nodename().to_string_lossy();
    let short = name.split('.').next().unwrap_or("");
    if short.is_empty() {
        "localhost".to_owned()
    } else {
        short.to_owned()
    }
}

pub fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

pub fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Name of the effective user: `$USER` if set, else the name from /etc/passwd, else the
/// numeric uid. Root is always "root".
pub fn username() -> String {
    if is_root() {
        return "root".to_owned();
    }
    if let Ok(u) = env::var("USER")
        && !u.is_empty()
        && !u.contains('/')
    {
        return u;
    }
    lookup_passwd_name(euid()).unwrap_or_else(|| euid().to_string())
}

fn lookup_passwd_name(uid: u32) -> Option<String> {
    let text = fs::read_to_string("/etc/passwd").ok()?;
    text.lines().find_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        let _pw = f.next()?;
        (f.next()?.parse::<u32>().ok()? == uid).then(|| name.to_owned())
    })
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone)]
pub struct Mount {
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub source: String,
}

/// Currently mounted filesystems.
pub fn mounts() -> Vec<Mount> {
    let Ok(text) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            // id parent maj:min root mount_point options [optional...] - fstype source superopts
            let (left, right) = line.split_once(" - ")?;
            let mount_point = unescape_mount(left.split(' ').nth(4)?);
            let mut r = right.split(' ');
            let fs_type = r.next()?.to_owned();
            let source = unescape_mount(r.next()?);
            Some(Mount {
                mount_point: PathBuf::from(mount_point),
                fs_type,
                source,
            })
        })
        .collect()
}

/// mountinfo escapes space, tab, newline and backslash as `\ooo` octal.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount that contains `path` (longest matching mount point).
pub fn mount_of(path: &Path) -> Option<Mount> {
    let path = fs::canonicalize(path).ok()?;
    mounts()
        .into_iter()
        .filter(|m| path.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.as_os_str().len())
}

/// Is `path` itself a mount point?
pub fn is_mount_point(path: &Path) -> bool {
    let Ok(path) = fs::canonicalize(path) else {
        return false;
    };
    mounts().iter().any(|m| m.mount_point == path)
}

/// Is the filesystem with this UUID mounted such that `path` lies on it?
pub fn path_is_on_fs_uuid(path: &Path, uuid: &str) -> bool {
    let Ok(dev) = fs::canonicalize(Path::new("/dev/disk/by-uuid").join(uuid)) else {
        return false;
    };
    let Some(m) = mount_of(path) else {
        return false;
    };
    fs::canonicalize(&m.source).is_ok_and(|s| s == dev)
}

/// Where the filesystem with this UUID is mounted right now, if anywhere.
pub fn mount_point_for_uuid(uuid: &str) -> Option<PathBuf> {
    let dev = fs::canonicalize(Path::new("/dev/disk/by-uuid").join(uuid)).ok()?;
    mounts()
        .into_iter()
        .filter(|m| fs::canonicalize(&m.source).is_ok_and(|s| s == dev))
        .map(|m| m.mount_point)
        .min_by_key(|p| p.as_os_str().len())
}

/// Best guess of the mount point for a removable-drive path that isn't mounted now:
/// `/run/media/<user>/<label>`, `/media/<user>/<label>`, `/media/<label>` or `/mnt/<label>`.
pub fn guess_mount_point(path: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = path.components().collect();
    let take = |n: usize| (parts.len() >= n).then(|| parts[..n].iter().collect::<PathBuf>());
    if path.starts_with("/run/media") {
        take(5)
    } else if path.starts_with("/media") {
        // /media/<user>/<label> (Debian/Ubuntu udisks) or /media/<label>
        let user_dir = take(3).filter(|p| p.file_name().is_some_and(|n| n == username().as_str()));
        if user_dir.is_some() { take(4) } else { take(3) }
    } else if path.starts_with("/mnt") {
        take(3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape() {
        assert_eq!(
            unescape_mount(r"/run/media/me/My\040Disk"),
            "/run/media/me/My Disk"
        );
        assert_eq!(unescape_mount(r"/plain"), "/plain");
        assert_eq!(unescape_mount(r"/trailing\04"), r"/trailing\04");
    }

    #[test]
    fn guesses_mount_points() {
        assert_eq!(
            guess_mount_point(Path::new("/run/media/me/Backup/flux")),
            Some(PathBuf::from("/run/media/me/Backup"))
        );
        assert_eq!(
            guess_mount_point(Path::new("/mnt/usb/x/y")),
            Some(PathBuf::from("/mnt/usb"))
        );
        assert_eq!(
            guess_mount_point(Path::new("/media/Disk")),
            Some(PathBuf::from("/media/Disk"))
        );
        assert_eq!(guess_mount_point(Path::new("/home/me/b")), None);
    }

    #[test]
    fn basics() {
        assert!(!hostname().is_empty());
        assert!(!username().is_empty());
        assert!(mount_of(Path::new("/")).is_some());
        assert!(is_mount_point(Path::new("/")));
    }
}
