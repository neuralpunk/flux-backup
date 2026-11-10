//! Human-friendly value types used in profiles: byte sizes ("2 GiB") and durations ("4h").

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A byte count. Parses and prints human-readable forms such as `512MiB`, `2 GB` or `1048576`.
///
/// Decimal suffixes (`kB`, `MB`, `GB`, `TB`) are powers of 1000. Binary suffixes (`KiB`,
/// `MiB`, `GiB`, `TiB`) are powers of 1024. A bare `K`/`M`/`G`/`T` is read as binary, which is
/// what most Linux tools mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub const fn bytes(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid size {input:?}: {reason}")]
pub struct ParseSizeError {
    input: String,
    reason: &'static str,
}

impl FromStr for ByteSize {
    type Err = ParseSizeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| ParseSizeError {
            input: s.to_owned(),
            reason,
        };
        let t = s.trim();
        let split = t
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(t.len());
        let (num, unit) = t.split_at(split);
        if num.is_empty() {
            return Err(err("missing number"));
        }
        let mult: u64 = match unit.trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "kb" => 1_000,
            "mb" => 1_000_000,
            "gb" => 1_000_000_000,
            "tb" => 1_000_000_000_000,
            "k" | "kib" => 1 << 10,
            "m" | "mib" => 1 << 20,
            "g" | "gib" => 1 << 30,
            "t" | "tib" => 1 << 40,
            _ => {
                return Err(err(
                    "unknown unit (use B, KiB, MiB, GiB, TiB, kB, MB, GB, TB)",
                ));
            }
        };
        let value = if num.contains('.') {
            let f: f64 = num.parse().map_err(|_| err("bad number"))?;
            let v = f * mult as f64;
            if !v.is_finite() || v < 0.0 || v > u64::MAX as f64 {
                return Err(err("out of range"));
            }
            v.round() as u64
        } else {
            let n: u64 = num.parse().map_err(|_| err("bad number"))?;
            n.checked_mul(mult).ok_or_else(|| err("out of range"))?
        };
        Ok(ByteSize(value))
    }
}

impl fmt::Display for ByteSize {
    /// Prints the unit, binary or decimal, that represents the value exactly with the
    /// smallest number ("500MB", "2GiB", "1TB"), so that `to_string()` → `parse()`
    /// round-trips without loss and reads the way it was most likely written.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [(&str, u64); 8] = [
            ("TiB", 1 << 40),
            ("TB", 1_000_000_000_000),
            ("GiB", 1 << 30),
            ("GB", 1_000_000_000),
            ("MiB", 1 << 20),
            ("MB", 1_000_000),
            ("KiB", 1 << 10),
            ("kB", 1_000),
        ];
        let best = UNITS
            .iter()
            .filter(|(_, m)| self.0 >= *m && self.0.is_multiple_of(*m))
            .min_by_key(|(_, m)| self.0 / m);
        match best {
            Some((name, m)) => write!(f, "{}{}", self.0 / m, name),
            None => write!(f, "{}B", self.0),
        }
    }
}

impl ByteSize {
    /// Approximate display for humans (`1.5 GiB`), not guaranteed to round-trip.
    pub fn display_approx(self) -> String {
        const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
        let mut v = self.0 as f64;
        let mut i = 0;
        while v >= 1024.0 && i < UNITS.len() - 1 {
            v /= 1024.0;
            i += 1;
        }
        if i == 0 {
            format!("{} B", self.0)
        } else {
            format!("{v:.1} {}", UNITS[i])
        }
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(u64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(n) => Ok(ByteSize(n)),
            Raw::Str(s) => s.parse().map_err(serde::de::Error::custom),
        }
    }
}

/// A duration written the human way: `90s`, `15m`, `4h`, `7d`, `1y 6months`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct HumanDuration(pub Duration);

impl HumanDuration {
    pub const fn from_secs(secs: u64) -> Self {
        HumanDuration(Duration::from_secs(secs))
    }
    pub const fn as_duration(self) -> Duration {
        self.0
    }
}

impl FromStr for HumanDuration {
    type Err = humantime::DurationError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        humantime::parse_duration(s.trim()).map(HumanDuration)
    }
}

impl fmt::Display for HumanDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", humantime::format_duration(self.0))
    }
}

impl Serialize for HumanDuration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HumanDuration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A share written as a percentage: `1%`, `0.5%`, `99.9%` (the sign is optional, and a
/// plain number is read the same way). Kept as millionths, so it compares exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percent(u32);

impl Percent {
    /// From millionths of the whole (`10_000` is 1%). More than the whole is the whole.
    pub const fn from_millionths(m: u32) -> Self {
        Percent(if m > 1_000_000 { 1_000_000 } else { m })
    }
    pub const fn millionths(self) -> u32 {
        self.0
    }
    /// As a fraction of one: 0.01 for 1%.
    pub fn fraction(self) -> f64 {
        f64::from(self.0) / 1e6
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid percentage {0:?} (write it like 1% or 99.9%)")]
pub struct ParsePercentError(String);

impl FromStr for Percent {
    type Err = ParsePercentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParsePercentError(s.to_owned());
        let t = s.trim();
        let t = t.strip_suffix('%').unwrap_or(t).trim();
        // Digits, with at most four after the point: read exactly, no rounding surprises.
        let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
        let digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() || !digits(whole) || !digits(frac) || frac.len() > 4 || whole.len() > 3
        {
            return Err(err());
        }
        let w: u32 = whole.parse().map_err(|_| err())?;
        let f: u32 = format!("{frac:0<4}").parse().map_err(|_| err())?;
        let m = w * 10_000 + f;
        if m > 1_000_000 {
            return Err(err());
        }
        Ok(Percent(m))
    }
}

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (w, frac) = (self.0 / 10_000, self.0 % 10_000);
        if frac == 0 {
            write!(f, "{w}%")
        } else {
            let digits = format!("{frac:04}");
            write!(f, "{w}.{}%", digits.trim_end_matches('0'))
        }
    }
}

impl Serialize for Percent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(u64),
            Float(f64),
            Str(String),
        }
        let text = match Raw::deserialize(d)? {
            Raw::Int(n) => n.to_string(),
            Raw::Float(x) => format!("{x}"),
            Raw::Str(s) => s,
        };
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages() {
        let p = |s: &str| s.parse::<Percent>().unwrap();
        assert_eq!(p("1%").millionths(), 10_000);
        assert_eq!(p(" 0.5 % ").millionths(), 5_000);
        assert_eq!(p("99.9").millionths(), 999_000);
        assert_eq!(p("0.01%").millionths(), 100);
        assert_eq!(p("100%").millionths(), 1_000_000);
        for bad in ["", "%", "101%", "1.23456%", "-1%", "1e2", "one"] {
            assert!(bad.parse::<Percent>().is_err(), "{bad}");
        }
        for s in ["1%", "0.5%", "99.9%", "99.99%", "0.01%", "100%", "0%"] {
            assert_eq!(p(s).to_string(), s);
        }
        // In a profile: a string, or a bare number.
        #[derive(Deserialize)]
        struct T {
            a: Percent,
            b: Percent,
            c: Percent,
        }
        let t: T = toml::from_str("a = \"1%\"\nb = 2\nc = 99.9").unwrap();
        assert_eq!((t.a, t.b, t.c), (p("1%"), p("2%"), p("99.9%")));
    }

    #[test]
    fn parses_sizes() {
        assert_eq!("0".parse::<ByteSize>().unwrap(), ByteSize(0));
        assert_eq!("1024".parse::<ByteSize>().unwrap(), ByteSize(1024));
        assert_eq!("2KiB".parse::<ByteSize>().unwrap(), ByteSize(2048));
        assert_eq!("2 kB".parse::<ByteSize>().unwrap(), ByteSize(2000));
        assert_eq!("1.5G".parse::<ByteSize>().unwrap(), ByteSize(3 << 29));
        assert_eq!("5 GB".parse::<ByteSize>().unwrap(), ByteSize(5_000_000_000));
        assert!("".parse::<ByteSize>().is_err());
        assert!("12 parsecs".parse::<ByteSize>().is_err());
        assert!("99999999999 TiB".parse::<ByteSize>().is_err());
    }

    #[test]
    fn size_display_round_trips() {
        for v in [
            0,
            1,
            1000,
            1024,
            1536,
            5 << 30,
            (1 << 40) * 3,
            123_456_789,
            500_000_000,
            1_000_000_000_000,
            1_500_000_000,
        ] {
            let b = ByteSize(v);
            assert_eq!(b.to_string().parse::<ByteSize>().unwrap(), b, "{b}");
        }
        assert_eq!(ByteSize(2 << 30).to_string(), "2GiB");
        // Decimal sizes read as written (the app's size limits use MB, GB, TB).
        assert_eq!(ByteSize(500_000_000).to_string(), "500MB");
        assert_eq!(ByteSize(1_000_000_000_000).to_string(), "1TB");
        assert_eq!(ByteSize(1).to_string(), "1B");
        assert_eq!(ByteSize(1536).display_approx(), "1.5 KiB");
    }

    #[test]
    fn durations() {
        let d: HumanDuration = "4h".parse().unwrap();
        assert_eq!(d.0, Duration::from_secs(4 * 3600));
        let d: HumanDuration = "1d 12h".parse().unwrap();
        assert_eq!(d.to_string().parse::<HumanDuration>().unwrap(), d);
        assert!("soon".parse::<HumanDuration>().is_err());
    }
}
