//! Network rules for backups to a server: which Wi-Fi networks scheduled backups may use,
//! and how fast they may upload at different times (full speed overnight, slower during
//! working hours).

use serde::{Deserialize, Serialize};

use crate::units::ByteSize;

/// `[network]` in a profile.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// Scheduled backups to a server run only on these Wi-Fi networks (by name). Empty:
    /// any. On a wired connection the Wi-Fi rules don't apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wifi_only: Vec<String>,
    /// Scheduled backups to a server never run on these Wi-Fi networks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wifi_never: Vec<String>,
    /// Upload limits by time of day; the first window that contains the time wins.
    /// Outside every window, the target's own `bandwidth_limit` applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bandwidth: Vec<BandwidthWindow>,
}

impl Network {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The upload limit at local time `t`: `default` (the target's limit) outside the
    /// windows; inside one, its limit (`None`: unlimited).
    pub fn limit_at(
        &self,
        t: &jiff::civil::DateTime,
        default: Option<ByteSize>,
    ) -> Option<ByteSize> {
        match self.bandwidth.iter().find(|w| w.contains(t)) {
            Some(w) => w.limit,
            None => default,
        }
    }

    /// Why a scheduled backup to a server may not run on Wi-Fi network `ssid` (`None`:
    /// not on Wi-Fi, so the rules don't apply).
    pub fn wifi_refuses(&self, ssid: Option<&str>) -> Option<String> {
        let ssid = ssid?;
        if self.wifi_never.iter().any(|n| n == ssid) {
            return Some(format!(
                "on the Wi-Fi network “{ssid}”, where backups don't run"
            ));
        }
        if !self.wifi_only.is_empty() && !self.wifi_only.iter().any(|n| n == ssid) {
            return Some(format!(
                "on the Wi-Fi network “{ssid}”, not one of {}",
                self.wifi_only
                    .iter()
                    .map(|n| format!("“{n}”"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        None
    }

    /// Problems with the windows, in words: (index, message).
    pub fn problems(&self) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (i, w) in self.bandwidth.iter().enumerate() {
            if parse_days(&w.days).is_none() {
                out.push((
                    i,
                    format!("days {:?}: use e.g. Mon..Fri, Sat,Sun or *", w.days),
                ));
            }
            for t in [&w.from, &w.to] {
                if parse_time(t).is_none() {
                    out.push((i, format!("time {t:?}: use HH:MM, e.g. 09:00")));
                }
            }
        }
        out
    }
}

/// An upload limit for part of the week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandwidthWindow {
    /// `*` (every day), `Mon..Fri`, `Sat,Sun`, `Mon,Wed..Fri`.
    #[serde(default = "every_day")]
    pub days: String,
    /// `HH:MM`, local time. A window can run past midnight (`22:00` to `06:00`).
    pub from: String,
    pub to: String,
    /// The upload limit then; missing: unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<ByteSize>,
}

fn every_day() -> String {
    "*".into()
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Days as a bit set (bit 0 = Monday).
fn parse_days(s: &str) -> Option<u8> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s == "*" {
        return Some(0x7f);
    }
    let day = |d: &str| {
        DAYS.iter()
            .position(|x| d.trim().starts_with(x) && d.trim().len() <= 9)
    };
    let mut set = 0u8;
    for part in s.split(',') {
        match part.split_once("..") {
            Some((a, b)) => {
                let (a, b) = (day(a)?, day(b)?);
                let mut i = a;
                loop {
                    set |= 1 << i;
                    if i == b {
                        break;
                    }
                    i = (i + 1) % 7;
                }
            }
            None => set |= 1 << day(part)?,
        }
    }
    Some(set)
}

/// Minutes since midnight.
fn parse_time(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl BandwidthWindow {
    /// Does the window contain local time `t`? A window past midnight belongs to the day
    /// it starts on.
    pub fn contains(&self, t: &jiff::civil::DateTime) -> bool {
        let (Some(days), Some(from), Some(to)) = (
            parse_days(&self.days),
            parse_time(&self.from),
            parse_time(&self.to),
        ) else {
            return false;
        };
        let wd = t.weekday().to_monday_zero_offset() as u32;
        let now = u32::from(t.hour() as u8) * 60 + u32::from(t.minute() as u8);
        let on = |d: u32| days & (1 << d) != 0;
        if from <= to {
            on(wd) && from <= now && now < to
        } else {
            // Past midnight: the evening part today, or the morning part of the day after.
            (on(wd) && now >= from) || (on((wd + 6) % 7) && now < to)
        }
    }
}

/// The Wi-Fi network this computer is connected to, from NetworkManager (`nmcli`; tests set
/// `FLUX_NMCLI` to a stand-in). `None` when not on Wi-Fi or it can't tell.
pub fn wifi_ssid() -> Option<String> {
    let nmcli = std::env::var_os("FLUX_NMCLI").unwrap_or_else(|| "nmcli".into());
    let out = std::process::Command::new(nmcli)
        .args(["-t", "-f", "ACTIVE,SSID", "device", "wifi"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| ssid_from(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

/// The active network in `nmcli -t` output (`yes:Home WiFi`; `:` in names is `\:`).
fn ssid_from(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let rest = l.strip_prefix("yes:")?;
        let ssid = rest.replace("\\:", ":").replace("\\\\", "\\");
        (!ssid.is_empty()).then_some(ssid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: i8, h: i8, m: i8) -> jiff::civil::DateTime {
        // 2026-11-09 is a Monday.
        jiff::civil::date(2026, 11, 9 + day).at(h, m, 0, 0)
    }

    fn w(days: &str, from: &str, to: &str, limit: Option<&str>) -> BandwidthWindow {
        BandwidthWindow {
            days: days.into(),
            from: from.into(),
            to: to.into(),
            limit: limit.map(|l| l.parse().unwrap()),
        }
    }

    #[test]
    fn windows_by_day_and_time() {
        let n = Network {
            bandwidth: vec![
                w("Mon..Fri", "09:00", "18:00", Some("2MB")),
                w("*", "22:00", "06:00", None),
            ],
            ..Default::default()
        };
        let default = Some("10MB".parse().unwrap());
        let mb = |s: &str| Some(s.parse::<ByteSize>().unwrap());
        assert_eq!(
            n.limit_at(&at(0, 10, 0), default),
            mb("2MB"),
            "Monday working hours"
        );
        assert_eq!(
            n.limit_at(&at(0, 18, 0), default),
            default,
            "18:00 is outside"
        );
        assert_eq!(n.limit_at(&at(5, 10, 0), default), default, "Saturday");
        assert_eq!(
            n.limit_at(&at(0, 23, 0), default),
            None,
            "overnight: unlimited"
        );
        assert_eq!(
            n.limit_at(&at(1, 5, 59), default),
            None,
            "still Monday night"
        );
        assert_eq!(n.limit_at(&at(1, 6, 0), default), default);
        assert!(n.problems().is_empty());
        let bad = Network {
            bandwidth: vec![w("Funday", "25:00", "x", None)],
            ..Default::default()
        };
        assert_eq!(bad.problems().len(), 3);
        assert_eq!(parse_days("Sat,Sun"), Some(0b110_0000));
        assert_eq!(
            parse_days("fri..mon"),
            Some(0b111_0001),
            "wraps round the week"
        );
    }

    #[test]
    fn wifi_rules() {
        let only = Network {
            wifi_only: vec!["Home".into(), "Office".into()],
            ..Default::default()
        };
        assert!(only.wifi_refuses(Some("Home")).is_none());
        assert!(
            only.wifi_refuses(Some("Café"))
                .unwrap()
                .contains("not one of “Home”, “Office”")
        );
        assert!(
            only.wifi_refuses(None).is_none(),
            "wired: rules don't apply"
        );
        let never = Network {
            wifi_never: vec!["Phone hotspot".into()],
            ..Default::default()
        };
        assert!(never.wifi_refuses(Some("Phone hotspot")).is_some());
        assert!(never.wifi_refuses(Some("Home")).is_none());
        assert_eq!(
            ssid_from("no:Other\nyes:Home WiFi\n").as_deref(),
            Some("Home WiFi")
        );
        assert_eq!(ssid_from("yes:a\\:b\n").as_deref(), Some("a:b"));
        assert_eq!(ssid_from("no:x\n"), None);
    }
}
