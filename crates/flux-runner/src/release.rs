//! "Safe to unplug": after a backup to a USB drive, unmount the drive and power it off,
//! through udisks (`udisksctl`), as the desktop's own *Eject* does.
//!
//! Only drives the desktop mounted (under `/run/media/` or `/media/`) are released: a
//! drive mounted by `/etc/fstab` or holding the system would need an administrator's
//! password, and a scheduled backup can't ask. An encrypted (LUKS) drive is locked again
//! after it's unmounted. A disk with another filesystem still mounted is only unmounted,
//! not powered off.

use std::fs;
use std::path::{Path, PathBuf};

use flux_core::sys::{self, Mount};

/// What will be done to release one drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Where the drive is mounted (what the person sees, e.g. `/run/media/ann/Backup`).
    pub mount_point: PathBuf,
    /// The block device to unmount (`/dev/sdb1`, `/dev/mapper/luks-…`).
    pub unmount: String,
    /// The encrypted partition to lock after unmounting (`/dev/sdb1`), for LUKS drives.
    pub lock: Option<String>,
    /// The whole disk to power off (`/dev/sdb`). `None` when something else on the same
    /// disk is still mounted: then it's only unmounted.
    pub power_off: Option<String>,
}

/// How a drive was left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Released {
    /// Unmounted and powered off: safe to unplug.
    PoweredOff,
    /// Unmounted (and locked), but not powered off; still safe to unplug.
    Unmounted,
}

/// Why a drive isn't released, in words for the run's log.
pub type Refusal = String;

/// Work out how to release the drive `path` (a backup's target folder) is on, from the
/// current mounts and sysfs (`/sys` normally; tests pass a fake one).
pub fn plan(path: &Path, mounts: &[Mount], sys_root: &Path) -> Result<Plan, Refusal> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mount = mounts
        .iter()
        .filter(|m| path.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.as_os_str().len())
        .ok_or_else(|| "its drive isn't mounted".to_string())?;
    plan_for_mount(mount, mounts, sys_root)
}

fn plan_for_mount(mount: &Mount, mounts: &[Mount], sys_root: &Path) -> Result<Plan, Refusal> {
    if !is_desktop_mount(&mount.mount_point) {
        return Err(format!(
            "{} isn't a drive the desktop mounted (under /run/media or /media), so it's left mounted",
            mount.mount_point.display()
        ));
    }
    if !mount.source.starts_with("/dev/") {
        return Err(format!("{} isn't on a drive", mount.mount_point.display()));
    }
    let dev = device_of(&mount.source, sys_root)
        .ok_or_else(|| format!("can't tell which disk {} is", mount.source))?;
    // Anything else mounted from the same disk keeps it powered.
    let shared = mounts.iter().any(|m| {
        m.mount_point != mount.mount_point
            && m.source.starts_with("/dev/")
            && device_of(&m.source, sys_root).is_some_and(|d| d.disk == dev.disk)
    });
    Ok(Plan {
        mount_point: mount.mount_point.clone(),
        unmount: mount.source.clone(),
        lock: dev.crypt_backing.map(|p| format!("/dev/{p}")),
        power_off: (!shared).then(|| format!("/dev/{}", dev.disk)),
    })
}

/// Mounted by the desktop (udisks), so the user may unmount it without a password.
fn is_desktop_mount(p: &Path) -> bool {
    p.starts_with("/run/media/") || p.starts_with("/media/")
}

/// A mounted device, in sysfs names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Device {
    /// The partition (or whole disk) the filesystem is on, under any encryption: `sdb1`.
    crypt_backing: Option<String>,
    /// The whole disk: `sdb`, `nvme1n1`.
    disk: String,
}

/// Which partition and disk a block device (`/dev/sdb1`, `/dev/mapper/luks-…`) is.
fn device_of(source: &str, sys_root: &Path) -> Option<Device> {
    let name = kernel_name(source)?;
    let class = sys_root.join("class/block");
    // A LUKS mapping (`dm-3`, uuid `CRYPT-LUKS2-…`): the partition under it.
    let dm_uuid = fs::read_to_string(class.join(&name).join("dm/uuid")).unwrap_or_default();
    let (part, crypt_backing) = if dm_uuid.starts_with("CRYPT-") {
        let slave = fs::read_dir(class.join(&name).join("slaves"))
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .next()?;
        (slave.clone(), Some(slave))
    } else if !dm_uuid.is_empty() {
        // LVM and other mappings span disks in ways not worth guessing.
        return None;
    } else {
        (name, None)
    };
    let part_dir = class.join(&part);
    let disk = if part_dir.join("partition").exists() {
        fs::canonicalize(&part_dir)
            .ok()?
            .parent()?
            .file_name()?
            .to_string_lossy()
            .into_owned()
    } else {
        part
    };
    Some(Device {
        crypt_backing,
        disk,
    })
}

/// The whole disk (`sdb`, `nvme0n1`) holding the filesystem `path` is on, under any
/// encryption. `None` for what isn't on one disk (LVM, network shares, tmpfs).
pub fn disk_of(path: &Path) -> Option<String> {
    let m = flux_core::sys::mount_of(path)?;
    if !m.source.starts_with("/dev/") {
        return None;
    }
    device_of(&m.source, Path::new("/sys")).map(|d| d.disk)
}

/// The kernel's name for a device node: `/dev/mapper/luks-x` → `dm-3`, `/dev/sdb1` → `sdb1`.
fn kernel_name(source: &str) -> Option<String> {
    let p = Path::new(source);
    let real = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    Some(real.file_name()?.to_string_lossy().into_owned())
}

/// The program that talks to udisks: `udisksctl`, or `FLUX_UDISKSCTL` (for tests).
fn udisksctl() -> String {
    std::env::var("FLUX_UDISKSCTL").unwrap_or_else(|_| "udisksctl".into())
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(args)
        .arg("--no-user-interaction")
        .output()
        .map_err(|e| format!("cannot run udisksctl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(if err.is_empty() {
        format!("udisksctl {} failed ({})", args[0], out.status)
    } else {
        // "Error unmounting /dev/sdb1: GDBus.Error:…: Target is busy" → the useful end.
        err.rsplit(": ").next().unwrap_or(&err).to_owned()
    })
}

/// Carry out a plan. Unmounting is what makes the drive safe; locking and powering off
/// follow when they can.
pub fn execute(plan: &Plan) -> Result<Released, String> {
    execute_with(&udisksctl(), plan)
}

fn execute_with(program: &str, plan: &Plan) -> Result<Released, String> {
    run(program, &["unmount", "-b", &plan.unmount])
        .map_err(|e| format!("couldn't unmount {}: {e}", plan.mount_point.display()))?;
    if let Some(l) = &plan.lock
        && let Err(e) = run(program, &["lock", "-b", l])
    {
        tracing::debug!("lock {l}: {e}");
        return Ok(Released::Unmounted);
    }
    match &plan.power_off {
        Some(d) => match run(program, &["power-off", "-b", d]) {
            Ok(()) => Ok(Released::PoweredOff),
            Err(e) => {
                tracing::debug!("power-off {d}: {e}");
                Ok(Released::Unmounted)
            }
        },
        None => Ok(Released::Unmounted),
    }
}

/// Release the drives behind `paths` (a backup's local targets), each once. Returns, per
/// drive, its mount point and how it went.
pub fn release_all<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
) -> Vec<(PathBuf, Result<Released, String>)> {
    let mounts = sys::mounts();
    let mut plans: Vec<Plan> = Vec::new();
    let mut out = Vec::new();
    for p in paths {
        match plan(p, &mounts, Path::new("/sys")) {
            Ok(pl) if plans.iter().any(|q| q.unmount == pl.unmount) => {}
            Ok(pl) => plans.push(pl),
            Err(e) => out.push((p.to_path_buf(), Err(e))),
        }
    }
    for pl in plans {
        let r = execute(&pl);
        out.push((pl.mount_point, r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn m(point: &str, source: &str) -> Mount {
        Mount {
            mount_point: point.into(),
            fs_type: "ext4".into(),
            source: source.into(),
        }
    }

    /// A fake sysfs: disk `sdb` with partitions `sdb1`, `sdb2`; `dm-3` a LUKS mapping on
    /// `sdb2`; `dm-4` an LVM volume; `sdc` a whole-disk filesystem.
    fn fake_sys() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let dev = t.path().join("devices/usb/sdb");
        for p in ["sdb1", "sdb2"] {
            fs::create_dir_all(dev.join(p)).unwrap();
            fs::write(dev.join(p).join("partition"), "1").unwrap();
        }
        fs::create_dir_all(t.path().join("devices/usb/sdc")).unwrap();
        let class = t.path().join("class/block");
        fs::create_dir_all(&class).unwrap();
        for p in ["sdb1", "sdb2"] {
            symlink(dev.join(p), class.join(p)).unwrap();
        }
        symlink(dev.clone(), class.join("sdb")).unwrap();
        symlink(t.path().join("devices/usb/sdc"), class.join("sdc")).unwrap();
        let dm3 = class.join("dm-3");
        fs::create_dir_all(dm3.join("dm")).unwrap();
        fs::write(dm3.join("dm/uuid"), "CRYPT-LUKS2-abc-luks-abc\n").unwrap();
        fs::create_dir_all(dm3.join("slaves/sdb2")).unwrap();
        let dm4 = class.join("dm-4");
        fs::create_dir_all(dm4.join("dm")).unwrap();
        fs::write(dm4.join("dm/uuid"), "LVM-xyz\n").unwrap();
        t
    }

    #[test]
    fn a_usb_partition_is_unmounted_and_its_disk_powered_off() {
        let sys = fake_sys();
        let mounts = [
            m("/", "/dev/nvme0n1p2"),
            m("/run/media/ann/Backup", "/dev/sdb1"),
        ];
        let p = plan(Path::new("/run/media/ann/Backup/flux"), &mounts, sys.path()).unwrap();
        assert_eq!(p.unmount, "/dev/sdb1");
        assert_eq!(p.lock, None);
        assert_eq!(p.power_off.as_deref(), Some("/dev/sdb"));
    }

    #[test]
    fn an_encrypted_drive_is_locked_too() {
        let sys = fake_sys();
        let mounts = [m("/run/media/ann/Secret", "/dev/dm-3")];
        let p = plan(Path::new("/run/media/ann/Secret"), &mounts, sys.path()).unwrap();
        assert_eq!(p.lock.as_deref(), Some("/dev/sdb2"));
        assert_eq!(p.power_off.as_deref(), Some("/dev/sdb"));
    }

    #[test]
    fn another_mounted_partition_keeps_the_disk_powered() {
        let sys = fake_sys();
        let mounts = [
            m("/run/media/ann/Backup", "/dev/sdb1"),
            m("/run/media/ann/Secret", "/dev/dm-3"),
        ];
        let p = plan(Path::new("/run/media/ann/Backup"), &mounts, sys.path()).unwrap();
        assert_eq!(p.power_off, None);
    }

    #[test]
    fn a_whole_disk_filesystem_is_its_own_disk() {
        let sys = fake_sys();
        let mounts = [m("/media/ann/Stick", "/dev/sdc")];
        let p = plan(Path::new("/media/ann/Stick"), &mounts, sys.path()).unwrap();
        assert_eq!(p.power_off.as_deref(), Some("/dev/sdc"));
    }

    #[test]
    fn drives_the_desktop_didnt_mount_are_left_alone() {
        let sys = fake_sys();
        let fstab = [m("/mnt/backup", "/dev/sdb1")];
        let e = plan(Path::new("/mnt/backup"), &fstab, sys.path()).unwrap_err();
        assert!(e.contains("isn't a drive the desktop mounted"), "{e}");
        let home = [m("/", "/dev/nvme0n1p2")];
        assert!(plan(Path::new("/home/ann/backups"), &home, sys.path()).is_err());
        let lvm = [m("/run/media/ann/Vol", "/dev/dm-4")];
        assert!(plan(Path::new("/run/media/ann/Vol"), &lvm, sys.path()).is_err());
        let net = [m("/run/media/ann/Share", "//nas/share")];
        assert!(plan(Path::new("/run/media/ann/Share"), &net, sys.path()).is_err());
    }

    /// A stand-in for udisksctl that logs its arguments and fails the commands named in
    /// `fail`.
    fn fake_udisksctl(dir: &Path, fail: &[&str]) -> String {
        let log = dir.join("calls");
        let script = dir.join("udisksctl");
        let cases: String = fail
            .iter()
            .map(|c| format!("  {c}) echo 'Error: GDBus.Error:x: Target is busy' >&2; exit 1;;\n"))
            .collect();
        // Never held open for writing by this multithreaded process (a child forked by
        // another test meanwhile would keep it busy): `install` creates the executable.
        let text = dir.join("udisksctl.txt");
        fs::write(
            &text,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1\" in\n{cases}esac\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        let ok = std::process::Command::new("install")
            .args(["-m", "0755"])
            .arg(&text)
            .arg(&script)
            .status()
            .unwrap();
        assert!(ok.success());
        script.display().to_string()
    }

    fn sample_plan() -> Plan {
        Plan {
            mount_point: "/run/media/ann/Secret".into(),
            unmount: "/dev/dm-3".into(),
            lock: Some("/dev/sdb2".into()),
            power_off: Some("/dev/sdb".into()),
        }
    }

    #[test]
    fn released_in_order_without_asking_for_a_password() {
        let t = tempfile::tempdir().unwrap();
        let prog = fake_udisksctl(t.path(), &[]);
        assert_eq!(
            execute_with(&prog, &sample_plan()),
            Ok(Released::PoweredOff)
        );
        let calls = fs::read_to_string(t.path().join("calls")).unwrap();
        assert_eq!(
            calls,
            "unmount -b /dev/dm-3 --no-user-interaction\n\
             lock -b /dev/sdb2 --no-user-interaction\n\
             power-off -b /dev/sdb --no-user-interaction\n"
        );
    }

    #[test]
    fn a_busy_drive_stays_and_says_why() {
        let t = tempfile::tempdir().unwrap();
        let prog = fake_udisksctl(t.path(), &["unmount"]);
        let e = execute_with(&prog, &sample_plan()).unwrap_err();
        assert_eq!(e, "couldn't unmount /run/media/ann/Secret: Target is busy");
    }

    #[test]
    fn unmounted_is_enough_when_power_off_fails() {
        let t = tempfile::tempdir().unwrap();
        let prog = fake_udisksctl(t.path(), &["power-off"]);
        assert_eq!(execute_with(&prog, &sample_plan()), Ok(Released::Unmounted));
    }
}
