//! systemd calendar expressions (`OnCalendar=`), read by Flux itself for the cron
//! scheduler: when is the next run, and the cron line that fires then.
//!
//! The common forms are understood: `daily`, `weekly`, `monthly`, `quarterly`, `yearly`,
//! `hourly`, `minutely`; `[WEEKDAYS] [[YEAR-]MONTH-DAY] [HH:MM[:SS]]` with lists (`7,19`),
//! ranges (`Mon..Fri`, `1..15`) and steps (`0/15`), as systemd reads them. Precision is a minute, like
//! cron's; seconds are read and ignored. Time zones and the last-day-of-month `~` are not
//! supported (systemd timers still take them).

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;

/// A set of small numbers (0..64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Set {
    bits: u64,
    /// Written as `*`: no restriction.
    any: bool,
}

impl Set {
    fn any(lo: u32, hi: u32) -> Self {
        let mut bits = 0;
        for v in lo..=hi {
            bits |= 1 << v;
        }
        Self { bits, any: true }
    }
    fn has(&self, v: u32) -> bool {
        v < 64 && self.bits & (1 << v) != 0
    }
    fn values(&self, lo: u32, hi: u32) -> impl Iterator<Item = u32> + '_ {
        (lo..=hi).filter(|v| self.has(*v))
    }
    /// Cron syntax: `*`, or a list with ranges (`1-5,7`).
    fn cron(&self, lo: u32, hi: u32) -> String {
        if self.any {
            return "*".into();
        }
        let vals: Vec<u32> = self.values(lo, hi).collect();
        let mut parts = Vec::new();
        let mut i = 0;
        while i < vals.len() {
            let start = vals[i];
            let mut end = start;
            while i + 1 < vals.len() && vals[i + 1] == end + 1 {
                i += 1;
                end = vals[i];
            }
            parts.push(if end > start + 1 {
                format!("{start}-{end}")
            } else if end == start + 1 {
                format!("{start},{end}")
            } else {
                start.to_string()
            });
            i += 1;
        }
        parts.join(",")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    /// Bit 0 = Monday … bit 6 = Sunday.
    weekdays: Set,
    /// Explicit years, if any.
    years: Option<Vec<(i16, i16)>>,
    months: Set,
    days: Set,
    hours: Set,
    minutes: Set,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct CalendarError(String);

fn err<T>(m: impl Into<String>) -> Result<T, CalendarError> {
    Err(CalendarError(m.into()))
}

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn weekday_index(s: &str) -> Option<u32> {
    const FULL: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let s = s.to_ascii_lowercase();
    (0..7)
        .find(|&i| s == WEEKDAYS[i] || s == FULL[i])
        .map(|i| i as u32)
}

/// One field: `*`, `5`, `1,3`, `1..5`, `*/2`, `0/15`, or lists of these.
fn field(s: &str, lo: u32, hi: u32, what: &str) -> Result<Set, CalendarError> {
    if s == "*" {
        return Ok(Set::any(lo, hi));
    }
    let num = |v: &str| -> Result<u32, CalendarError> {
        match v.parse::<u32>() {
            Ok(n) if (lo..=hi).contains(&n) => Ok(n),
            _ => err(format!("{v:?} is not a valid {what}")),
        }
    };
    let mut bits = 0u64;
    for part in s.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, st)) => match st.parse::<u32>() {
                Ok(n) if n > 0 => (r, n),
                _ => return err(format!("{st:?} is not a valid step")),
            },
            None => (part, 0),
        };
        let (a, b) = if range == "*" {
            if step > 0 {
                // systemd writes a step from the start: "0/15", not "*/15".
                return err(format!("{part:?}: write steps from a start, like 0/{step}"));
            }
            (lo, hi)
        } else if let Some((a, b)) = range.split_once("..") {
            (num(a)?, num(b)?)
        } else {
            let a = num(range)?;
            (a, if step > 0 { hi } else { a })
        };
        if a > b {
            return err(format!("{part:?}: the range runs backwards"));
        }
        let step = step.max(1);
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok(Set { bits, any: false })
}

fn weekdays(s: &str) -> Result<Set, CalendarError> {
    let mut bits = 0u64;
    for part in s.split(',') {
        let (a, b) = match part.split_once("..").or_else(|| part.split_once('-')) {
            Some((a, b)) => (a, b),
            None => (part, part),
        };
        let (Some(a), Some(b)) = (weekday_index(a), weekday_index(b)) else {
            return err(format!("{part:?} is not a weekday"));
        };
        if a > b {
            // As systemd: "Fri..Mon" is refused, "Fri..Sun,Mon" isn't.
            return err(format!("{part:?}: the range runs backwards"));
        }
        for v in a..=b {
            bits |= 1 << v;
        }
    }
    Ok(Set { bits, any: false })
}

impl Calendar {
    pub fn parse(expr: &str) -> Result<Self, CalendarError> {
        let lower = expr.trim().to_ascii_lowercase();
        let expanded = match lower.as_str() {
            "minutely" => "*-*-* *:*:00",
            "hourly" => "*-*-* *:00:00",
            "daily" => "*-*-* 00:00:00",
            "weekly" => "mon *-*-* 00:00:00",
            "monthly" => "*-*-01 00:00:00",
            "quarterly" => "*-01,04,07,10-01 00:00:00",
            "semiannually" | "semi-annually" => "*-01,07-01 00:00:00",
            "yearly" | "annually" => "*-01-01 00:00:00",
            other => other,
        };
        let mut cal = Calendar {
            weekdays: Set::any(0, 6),
            years: None,
            months: Set::any(1, 12),
            days: Set::any(1, 31),
            hours: Set {
                bits: 1,
                any: false,
            },
            minutes: Set {
                bits: 1,
                any: false,
            },
        };
        let tokens: Vec<&str> = expanded.split_whitespace().collect();
        if tokens.is_empty() {
            return err("the schedule is empty");
        }
        let (mut saw_date, mut saw_time) = (false, false);
        for (i, t) in tokens.iter().enumerate() {
            if t.contains(':') && !saw_time {
                saw_time = true;
                let parts: Vec<&str> = t.split(':').collect();
                if !(2..=3).contains(&parts.len()) {
                    return err(format!("{t:?} is not a time"));
                }
                cal.hours = field(parts[0], 0, 23, "hour")?;
                cal.minutes = field(parts[1], 0, 59, "minute")?;
                if let Some(s) = parts.get(2) {
                    // Read so a typo is still caught; precision is a minute.
                    let sec = s.split('.').next().unwrap_or(s);
                    field(sec, 0, 59, "second")?;
                }
            } else if t.contains('-')
                && t.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || c == '*')
                && !saw_date
            {
                saw_date = true;
                if t.contains('~') {
                    return err("the last day of a month (~) isn't supported here");
                }
                let parts: Vec<&str> = t.split('-').collect();
                let (y, m, d) = match parts.as_slice() {
                    [y, m, d] => (Some(*y), *m, *d),
                    [m, d] => (None, *m, *d),
                    _ => return err(format!("{t:?} is not a date")),
                };
                if let Some(y) = y.filter(|y| *y != "*") {
                    let mut ranges = Vec::new();
                    for part in y.split(',') {
                        let year = |v: &str| v.parse::<i16>().ok().filter(|y| *y >= 1970);
                        let r = match part.split_once("..") {
                            Some((a, b)) => year(a).zip(year(b)),
                            None => year(part).map(|a| (a, a)),
                        };
                        match r {
                            Some(r) => ranges.push(r),
                            None => return err(format!("{part:?} is not a year")),
                        }
                    }
                    cal.years = Some(ranges);
                }
                cal.months = field(m, 1, 12, "month")?;
                cal.days = field(d, 1, 31, "day")?;
            } else if i == 0 && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                cal.weekdays = weekdays(t)?;
            } else {
                return err(format!(
                    "{t:?} isn't understood here (time zones aren't supported with cron)"
                ));
            }
        }
        Ok(cal)
    }

    fn day_matches(&self, d: Date) -> bool {
        let wd = d.weekday().to_monday_zero_offset() as u32;
        self.weekdays.has(wd)
            && self.months.has(d.month() as u32)
            && self.days.has(d.day() as u32)
            && self
                .years
                .as_ref()
                .is_none_or(|ys| ys.iter().any(|(a, b)| (*a..=*b).contains(&d.year())))
    }

    /// The first time strictly after `after`, at minute precision (within eight years).
    pub fn next_civil(&self, after: DateTime) -> Option<DateTime> {
        let start = after
            .with()
            .second(0)
            .subsec_nanosecond(0)
            .build()
            .ok()?
            .checked_add(jiff::Span::new().minutes(1))
            .ok()?;
        let mut day = start.date();
        for _ in 0..(366 * 8) {
            if self.day_matches(day) {
                let from = if day == start.date() {
                    (start.hour() as u32, start.minute() as u32)
                } else {
                    (0, 0)
                };
                for h in self.hours.values(from.0, 23) {
                    let m0 = if h == from.0 { from.1 } else { 0 };
                    if let Some(m) = self.minutes.values(m0, 59).next() {
                        return Some(day.to_datetime(Time::new(h as i8, m as i8, 0, 0).ok()?));
                    }
                }
            }
            day = day.tomorrow().ok()?;
        }
        None
    }

    /// The first run strictly after `after`, in time zone `tz`.
    pub fn next_after(&self, after: Timestamp, tz: &TimeZone) -> Option<Timestamp> {
        let local = after.to_zoned(tz.clone()).datetime();
        let mut next = self.next_civil(local)?;
        // A time skipped by a DST change resolves to the moment after the gap; a repeated
        // one to its first occurrence. Either way it must be after `after`.
        for _ in 0..4 {
            let t = tz.to_ambiguous_zoned(next).compatible().ok()?.timestamp();
            if t > after {
                return Some(t);
            }
            next = self.next_civil(next)?;
        }
        None
    }

    /// The five cron fields that fire at (at least) these times. When both the day of the
    /// month and the weekday are restricted, cron would take either; the line then names
    /// just the weekdays, and Flux's own check (`--if-due`) skips the other days. Years
    /// are checked the same way.
    pub fn cron_fields(&self) -> String {
        let dow = if self.weekdays.bits == Set::any(0, 6).bits {
            "*".to_owned()
        } else {
            // cron: 0 = Sunday … 6 = Saturday.
            let mut bits = 0u64;
            for v in self.weekdays.values(0, 6) {
                bits |= 1 << ((v + 1) % 7);
            }
            Set { bits, any: false }.cron(0, 6)
        };
        let dom = if dow != "*" {
            "*".to_owned()
        } else {
            self.days.cron(1, 31)
        };
        format!(
            "{} {} {dom} {} {dow}",
            self.minutes.cron(0, 59),
            self.hours.cron(0, 23),
            self.months.cron(1, 12)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::datetime;

    fn next(expr: &str, after: DateTime) -> DateTime {
        Calendar::parse(expr).unwrap().next_civil(after).unwrap()
    }

    #[test]
    fn shorthands_and_daily_times() {
        let t = datetime(2026, 11, 9, 8, 30, 15, 0); // a Monday
        assert_eq!(next("daily", t), datetime(2026, 11, 10, 0, 0, 0, 0));
        assert_eq!(next("hourly", t), datetime(2026, 11, 9, 9, 0, 0, 0));
        assert_eq!(next("weekly", t), datetime(2026, 11, 16, 0, 0, 0, 0));
        assert_eq!(next("monthly", t), datetime(2026, 12, 1, 0, 0, 0, 0));
        assert_eq!(next("quarterly", t), datetime(2027, 1, 1, 0, 0, 0, 0));
        assert_eq!(next("yearly", t), datetime(2027, 1, 1, 0, 0, 0, 0));
        assert_eq!(
            next("*-*-* 07:00:00", t),
            datetime(2026, 11, 10, 7, 0, 0, 0)
        );
        assert_eq!(next("*-*-* 09:15", t), datetime(2026, 11, 9, 9, 15, 0, 0));
        // Strictly after: the minute itself doesn't count.
        let at = datetime(2026, 11, 9, 7, 0, 0, 0);
        assert_eq!(
            next("*-*-* 07:00:00", at),
            datetime(2026, 11, 10, 7, 0, 0, 0)
        );
    }

    #[test]
    fn weekdays_lists_ranges_and_steps() {
        let fri = datetime(2026, 11, 13, 19, 0, 0, 0);
        assert_eq!(
            next("Mon..Fri 18:00", fri),
            datetime(2026, 11, 16, 18, 0, 0, 0)
        );
        assert_eq!(
            next("Sat,Sun 10:00", fri),
            datetime(2026, 11, 14, 10, 0, 0, 0)
        );
        assert_eq!(
            next("*-*-* 07,19:00:00", datetime(2026, 11, 9, 8, 0, 0, 0)),
            datetime(2026, 11, 9, 19, 0, 0, 0)
        );
        assert_eq!(
            next("*-*-* 0/4:00", datetime(2026, 11, 9, 8, 0, 0, 0)),
            datetime(2026, 11, 9, 12, 0, 0, 0)
        );
        assert_eq!(
            next("*:0/15", datetime(2026, 11, 9, 8, 50, 0, 0)),
            datetime(2026, 11, 9, 9, 0, 0, 0)
        );
        assert_eq!(
            next("Mon *-*-1..7 03:00", datetime(2026, 11, 9, 0, 0, 0, 0)),
            datetime(2026, 12, 7, 3, 0, 0, 0),
            "the first Monday of the month"
        );
        assert_eq!(
            next("*-02-29 00:00", datetime(2026, 1, 1, 0, 0, 0, 0)),
            datetime(2028, 2, 29, 0, 0, 0, 0),
            "the next leap day"
        );
    }

    #[test]
    fn impossible_and_bad_expressions() {
        assert!(
            Calendar::parse("*-02-30")
                .unwrap()
                .next_civil(datetime(2026, 1, 1, 0, 0, 0, 0))
                .is_none()
        );
        for bad in [
            "",
            "*-13-01",
            "25:00",
            "Funday",
            "daily UTC",
            "*-*~01",
            "*-*-* 07:00 extra",
            "10..2:00",
            "Fri..Mon 10:00",
            "*-*-* */4:00",
        ] {
            assert!(Calendar::parse(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn cron_lines() {
        let c = |e: &str| Calendar::parse(e).unwrap().cron_fields();
        assert_eq!(c("*-*-* 07:00:00"), "0 7 * * *");
        assert_eq!(c("daily"), "0 0 * * *");
        assert_eq!(c("hourly"), "0 * * * *");
        assert_eq!(c("Mon..Fri 18:00"), "0 18 * * 1-5");
        assert_eq!(c("Sun 10:30"), "30 10 * * 0");
        assert_eq!(c("*-*-* 07,19:00"), "0 7,19 * * *");
        assert_eq!(c("monthly"), "0 0 1 * *");
        assert_eq!(c("quarterly"), "0 0 1 1,4,7,10 *");
        assert_eq!(c("*:0/15"), "0,15,30,45 * * * *");
        // Day and weekday together: the weekday only; Flux's check does the rest.
        assert_eq!(c("Mon *-*-1..7 03:00"), "0 3 * * 1");
    }

    /// The same answers as systemd, where it's installed: the next five runs after a few
    /// moments, in UTC (`systemd-analyze calendar --iterations`).
    #[test]
    fn agrees_with_systemd() {
        let exprs = [
            "daily",
            "hourly",
            "weekly",
            "monthly",
            "quarterly",
            "semiannually",
            "yearly",
            "*-*-* 07:00:00",
            "Mon..Fri 18:00",
            "Sat,Sun 10:30",
            "*-*-* 07,19:00",
            "*-*-* 0/4:00",
            "*:0/15",
            "Mon *-*-1..7 03:00",
            "*-*-01/2 00:00",
            "*-02-29 00:00",
            "Wed 2027-*-* 12:00",
            "*-*-* 1..5:00",
        ];
        let tz = TimeZone::UTC;
        for base in [1_794_200_000i64, 1_800_000_000, 1_830_297_600] {
            for e in exprs {
                let Ok(out) = std::process::Command::new("systemd-analyze")
                    .env("TZ", "UTC")
                    .args([
                        "calendar",
                        "--iterations=5",
                        &format!("--base-time=@{base}"),
                        e,
                    ])
                    .output()
                else {
                    return; // no systemd here
                };
                let stdout = String::from_utf8_lossy(&out.stdout);
                // "Next elapse: …", then "Iteration #2: …" (systemd 250 and later) or
                // "Iter. #2: …" (older, such as Ubuntu 22.04's 249).
                let theirs: Vec<Timestamp> = stdout
                    .lines()
                    .filter_map(|l| {
                        let l = l.trim();
                        l.strip_prefix("Next elapse:").or_else(|| {
                            l.starts_with("Iter")
                                .then(|| l.split_once(':').map(|x| x.1))
                                .flatten()
                        })
                    })
                    .filter_map(|v| {
                        let v = v.trim().trim_end_matches(" UTC");
                        let dt: DateTime = v.get(4..)?.parse().ok()?;
                        Some(tz.to_ambiguous_zoned(dt).compatible().ok()?.timestamp())
                    })
                    .collect();
                let cal = Calendar::parse(e).unwrap();
                if theirs.is_empty() {
                    // Never again (a year that's over): both must say so.
                    let after = Timestamp::from_second(base).unwrap();
                    assert_eq!(cal.next_after(after, &tz), None, "{e} after @{base}");
                    assert!(out.status.success(), "{e}: systemd-analyze refused it");
                    continue;
                }
                assert_eq!(
                    theirs.len(),
                    5,
                    "{e}: systemd-analyze output not understood:\n{stdout}"
                );
                let mut ours = Vec::new();
                let mut t = Timestamp::from_second(base).unwrap();
                while ours.len() < theirs.len() {
                    t = cal.next_after(t, &tz).unwrap();
                    ours.push(t);
                }
                assert_eq!(ours, theirs, "{e} after @{base}");
            }
        }
    }

    #[test]
    fn next_after_in_a_zone() {
        let tz = TimeZone::get("America/Chicago").unwrap();
        let after: Timestamp = "2026-11-09T14:00:00Z".parse().unwrap(); // 08:00 local
        let n = Calendar::parse("*-*-* 07:00")
            .unwrap()
            .next_after(after, &tz)
            .unwrap();
        assert_eq!(n.to_string(), "2026-11-10T13:00:00Z");
    }
}
