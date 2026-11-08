//! Power state, from `/sys/class/power_supply`.

use std::fs;
use std::path::Path;

/// True if the machine has a battery and no external power supply is online.
/// Machines without batteries (desktops, servers) are never "on battery".
pub fn on_battery() -> bool {
    on_battery_in(Path::new("/sys/class/power_supply"))
}

fn on_battery_in(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut has_battery = false;
    let mut external_online = false;
    for e in entries.flatten() {
        let p = e.path();
        let read = |name: &str| {
            fs::read_to_string(p.join(name))
                .map(|s| s.trim().to_owned())
                .unwrap_or_default()
        };
        match read("type").as_str() {
            "Battery" => {
                // Peripheral batteries (mice, headsets) report scope "Device".
                if read("scope") != "Device" {
                    has_battery = true;
                }
            }
            "Mains" | "USB" | "USB_C" | "USB_PD" | "USB_PD_DRP" if read("online") == "1" => {
                external_online = true;
            }
            _ => {}
        }
    }
    has_battery && !external_online
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn detection() {
        let t = tempfile::tempdir().unwrap();
        assert!(!on_battery_in(t.path()), "desktop without supplies");
        supply(t.path(), "BAT0", &[("type", "Battery")]);
        supply(t.path(), "AC", &[("type", "Mains"), ("online", "1")]);
        assert!(!on_battery_in(t.path()), "laptop plugged in");
        fs::write(t.path().join("AC/online"), "0\n").unwrap();
        assert!(on_battery_in(t.path()), "laptop unplugged");
        supply(
            t.path(),
            "ucsi-source-psy-USBC000:001",
            &[("type", "USB"), ("online", "1")],
        );
        assert!(!on_battery_in(t.path()), "charging over USB-C");

        let t = tempfile::tempdir().unwrap();
        supply(
            t.path(),
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device")],
        );
        assert!(!on_battery_in(t.path()), "only a wireless mouse battery");
    }
}

/// Whether NetworkManager says the connection is metered (mobile data, a phone's hotspot,
/// or one the user marked so). Unknown counts as not metered.
pub fn metered() -> bool {
    let out = std::process::Command::new("busctl")
        .args([
            "--system",
            "get-property",
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
            "Metered",
        ])
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => metered_from(&String::from_utf8_lossy(&o.stdout)),
        _ => false,
    }
}

/// `busctl` prints `u 1`: NetworkManager's NMMetered (1 yes, 3 guessed yes; 0 unknown,
/// 2 no, 4 guessed no).
fn metered_from(busctl: &str) -> bool {
    matches!(busctl.trim().strip_prefix("u "), Some("1" | "3"))
}

#[cfg(test)]
mod metered_tests {
    #[test]
    fn reads_network_manager() {
        assert!(super::metered_from("u 1\n"));
        assert!(super::metered_from("u 3"));
        for no in ["u 0", "u 2", "u 4", "", "garbage"] {
            assert!(!super::metered_from(no), "{no}");
        }
    }
}
