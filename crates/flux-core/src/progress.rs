//! Live progress reporting shared by engines, the CLI and the GUI.
//!
//! Engines update lock-free counters in [`Progress`]. Front-ends poll [`Progress::snapshot`]
//! at whatever rate suits them, so the copying hot path never blocks on a slow consumer.
//! Rare events (phase changes, per-file warnings) go through a [`Reporter`] callback.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Phase {
    Preparing = 0,
    Scanning = 1,
    Planning = 2,
    Transferring = 3,
    Finalizing = 4,
    Done = 5,
    /// Waiting for another backup to finish ("one backup at a time").
    Waiting = 6,
}

impl Phase {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Phase::Scanning,
            2 => Phase::Planning,
            3 => Phase::Transferring,
            4 => Phase::Finalizing,
            5 => Phase::Done,
            6 => Phase::Waiting,
            _ => Phase::Preparing,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Preparing => "Preparing",
            Phase::Scanning => "Scanning",
            Phase::Planning => "Comparing",
            Phase::Transferring => "Copying",
            Phase::Finalizing => "Finalizing",
            Phase::Done => "Done",
            Phase::Waiting => "Waiting",
        }
    }
}

/// Shared progress counters. Cheap to update from many threads.
#[derive(Debug)]
pub struct Progress {
    phase: AtomicU8,
    /// Entries found while scanning.
    pub scanned: AtomicU64,
    /// Entries to process in the transfer phase / processed so far.
    pub entries_total: AtomicU64,
    pub entries_done: AtomicU64,
    /// Bytes that must actually be copied / copied so far.
    pub bytes_total: AtomicU64,
    pub bytes_done: AtomicU64,
    pub warnings: AtomicU64,
    cancel: AtomicBool,
    started: Instant,
    phase_started: Mutex<Instant>,
    /// The copy rate as last worked out, recent speed counting most.
    rate: Mutex<Rate>,
    /// Where the time goes, for the performance inspector.
    pub timings: Timings,
}

/// The copy rate, smoothed: when it was last worked out, from how many bytes, and what
/// it came to (bytes/second; 0 before the first look).
#[derive(Debug, Default, Clone, Copy)]
struct Rate {
    at: Option<Instant>,
    bytes: u64,
    per_sec: f64,
}

/// How quickly an older speed stops counting: after this long, about a third of it is
/// left. Long enough that a few slow seconds (a big file's first read, a busy disk) don't
/// swing the estimate, short enough that it follows a real change within a minute.
const RATE_MEMORY: Duration = Duration::from_secs(20);
/// Below this, a new look reuses the last rate (consumers poll five times a second).
const RATE_STEP: Duration = Duration::from_secs(1);

/// The smoothed rate after `secs` seconds at `recent` bytes/second, the rate before being
/// `before` (an exponential moving average over time, so it doesn't depend on how often
/// anyone looks).
fn smooth(before: f64, recent: f64, secs: f64) -> f64 {
    let keep = (-secs / RATE_MEMORY.as_secs_f64()).exp();
    before * keep + recent * (1.0 - keep)
}

/// A stage of the work, timed across threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading files (and, for big files, cutting them into chunks as they're read).
    Read,
    /// Cutting files into chunks.
    Chunk,
    /// Hashing chunks to find the ones already stored (deduplication).
    Dedup,
    /// Compressing and encrypting new chunks.
    Compress,
    /// Writing to the backup drive or uploading to the server.
    Write,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Read,
        Stage::Chunk,
        Stage::Dedup,
        Stage::Compress,
        Stage::Write,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Read => "Reading files",
            Stage::Chunk => "Cutting into chunks",
            Stage::Dedup => "Finding what's already stored",
            Stage::Compress => "Compressing and encrypting",
            Stage::Write => "Writing and uploading",
        }
    }
}

/// Time spent per stage (summed over threads, so it can exceed the run's length), the
/// wall-clock time of each phase, and the storage's writes.
#[derive(Debug, Default)]
pub struct Timings {
    stages: [AtomicU64; 5],
    /// Writes to the storage, and the bytes in them.
    pub writes: AtomicU64,
    pub bytes_written: AtomicU64,
    phases: Mutex<Vec<(String, f64)>>,
}

impl Timings {
    pub fn add(&self, stage: Stage, d: std::time::Duration) {
        let i = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
        self.stages[i].fetch_add(d.as_nanos().min(u128::from(u64::MAX)) as u64, Relaxed);
    }

    /// Run `f`, counting its time to `stage`.
    pub fn time<T>(&self, stage: Stage, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let out = f();
        self.add(stage, t.elapsed());
        out
    }

    /// A write of `bytes` that took `d`.
    pub fn wrote(&self, bytes: u64, d: std::time::Duration) {
        self.add(Stage::Write, d);
        self.writes.fetch_add(1, Relaxed);
        self.bytes_written.fetch_add(bytes, Relaxed);
    }

    /// Seconds per stage (stages never timed are left out).
    pub fn stages(&self) -> Vec<(Stage, f64)> {
        Stage::ALL
            .iter()
            .zip(&self.stages)
            .map(|(s, n)| (*s, n.load(Relaxed) as f64 / 1e9))
            .filter(|(_, secs)| *secs > 0.0)
            .collect()
    }

    /// Seconds per phase, in order (a phase entered twice adds up), with `extra` steps
    /// (a restore test, the unusual-change check) the runner times itself.
    pub fn phases(&self) -> Vec<(String, f64)> {
        self.phases.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Put the phases back as they were (after something reused the progress, like a
    /// restore test after a backup).
    pub fn restore_phases(&self, phases: Vec<(String, f64)>) {
        if let Ok(mut p) = self.phases.lock() {
            *p = phases;
        }
    }

    pub fn record_phase(&self, name: &str, secs: f64) {
        if let Ok(mut p) = self.phases.lock() {
            match p.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 += secs,
                None => p.push((name.to_owned(), secs)),
            }
        }
    }
}

impl Default for Progress {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            phase: AtomicU8::new(Phase::Preparing as u8),
            scanned: AtomicU64::new(0),
            entries_total: AtomicU64::new(0),
            entries_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            warnings: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            started: now,
            phase_started: Mutex::new(now),
            rate: Mutex::new(Rate::default()),
            timings: Timings::default(),
        }
    }
}

/// A consistent-enough copy of the counters for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub phase: Phase,
    pub scanned: u64,
    pub entries_total: u64,
    pub entries_done: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub warnings: u64,
    pub elapsed_secs: f64,
    /// Copy rate during the transfer phase, bytes/second: recent speed counts most
    /// (about the last 20 seconds), so a slow start doesn't hold the estimate down.
    pub bytes_per_sec: f64,
    /// Estimated seconds remaining in the transfer phase, at that rate.
    pub eta_secs: Option<f64>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_phase(&self, p: Phase) {
        let before = Phase::from_u8(self.phase.swap(p as u8, Relaxed));
        if let Ok(mut t) = self.phase_started.lock() {
            let spent = t.elapsed().as_secs_f64();
            if before != p && before != Phase::Done {
                self.timings.record_phase(before.label(), spent);
            }
            if before != p {
                *t = Instant::now();
                if let Ok(mut r) = self.rate.lock() {
                    *r = Rate::default();
                }
            }
        }
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Relaxed))
    }

    /// Ask the running job to stop at the next safe point.
    pub fn cancel(&self) {
        self.cancel.store(true, Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }

    /// The copy rate now, `bytes_done` having been copied in `phase_secs`: at first the
    /// average so far, then smoothed at most once a second.
    fn rate_now(&self, bytes_done: u64, phase_secs: f64) -> f64 {
        let average = bytes_done as f64 / phase_secs;
        let Ok(mut r) = self.rate.lock() else {
            return average;
        };
        let now = Instant::now();
        match r.at {
            None => {
                *r = Rate {
                    at: Some(now),
                    bytes: bytes_done,
                    per_sec: average,
                };
            }
            Some(at) if now.duration_since(at) >= RATE_STEP => {
                let secs = now.duration_since(at).as_secs_f64();
                let recent = bytes_done.saturating_sub(r.bytes) as f64 / secs;
                r.per_sec = smooth(r.per_sec, recent, secs);
                r.at = Some(now);
                r.bytes = bytes_done;
            }
            Some(_) => {}
        }
        r.per_sec
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let phase = self.phase();
        let bytes_total = self.bytes_total.load(Relaxed);
        let bytes_done = self.bytes_done.load(Relaxed);
        let phase_secs = self
            .phase_started
            .lock()
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or_default();
        let (rate, eta) = if phase == Phase::Transferring && phase_secs > 0.5 && bytes_done > 0 {
            let rate = self.rate_now(bytes_done, phase_secs);
            (
                rate,
                (rate > 0.0).then(|| bytes_total.saturating_sub(bytes_done) as f64 / rate),
            )
        } else {
            (0.0, None)
        };
        ProgressSnapshot {
            phase,
            scanned: self.scanned.load(Relaxed),
            entries_total: self.entries_total.load(Relaxed),
            entries_done: self.entries_done.load(Relaxed),
            bytes_total,
            bytes_done,
            warnings: self.warnings.load(Relaxed),
            elapsed_secs: self.started.elapsed().as_secs_f64(),
            bytes_per_sec: rate,
            eta_secs: eta,
        }
    }
}

/// Something a front-end may want to show as it happens.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Phase {
        phase: Phase,
    },
    /// A file or directory could not be backed up / restored; the job continues.
    Warning {
        path: String,
        message: String,
    },
    Info {
        message: String,
    },
}

/// Callback for [`Event`]s. Must be cheap and thread-safe.
pub type Reporter<'a> = &'a (dyn Fn(Event) + Sync);

/// A reporter that discards everything.
pub fn silent(_: Event) {}

/// Time left in words, as the app and the tray say it: "less than a minute left", "about
/// 6 minutes left", "about 1 hour 10 minutes left". Rounded, so it doesn't tick every second.
pub fn time_left_words(secs: f64) -> String {
    let plural = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    if secs < 60.0 {
        return "less than a minute left".into();
    }
    let mins = (secs / 60.0).round() as u64;
    if mins < 60 {
        return format!("about {} left", plural(mins, "minute"));
    }
    let (h, m) = (mins / 60, mins % 60);
    if m == 0 || h >= 10 {
        format!("about {} left", plural(h, "hour"))
    } else {
        format!("about {} {} left", plural(h, "hour"), plural(m, "minute"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_in_words() {
        assert_eq!(time_left_words(12.0), "less than a minute left");
        assert_eq!(time_left_words(70.0), "about 1 minute left");
        assert_eq!(time_left_words(359.0), "about 6 minutes left");
        assert_eq!(time_left_words(3600.0), "about 1 hour left");
        assert_eq!(time_left_words(4210.0), "about 1 hour 10 minutes left");
        assert_eq!(time_left_words(11.5 * 3600.0), "about 11 hours left");
    }

    #[test]
    fn the_rate_follows_recent_speed_whatever_the_polling() {
        // Ten slow seconds (1 MB/s), then 100 MB/s: after a minute it's nearly there.
        let mut r = 1e6;
        for _ in 0..60 {
            r = smooth(r, 100e6, 1.0);
        }
        assert!(r > 90e6, "{r}");
        // The same minute seen in six looks comes to the same.
        let mut s = 1e6;
        for _ in 0..6 {
            s = smooth(s, 100e6, 10.0);
        }
        assert!((r - s).abs() / r < 1e-9, "{r} {s}");
        // A few slow seconds don't swing it.
        let dip = smooth(100e6, 1e6, 3.0);
        assert!(dip > 80e6, "{dip}");
    }

    #[test]
    fn time_left_is_known_only_while_copying() {
        let p = Progress::new();
        p.bytes_total.store(1000, Relaxed);
        assert_eq!(p.snapshot().eta_secs, None);
        p.set_phase(Phase::Transferring);
        std::thread::sleep(Duration::from_millis(600));
        p.bytes_done.store(500, Relaxed);
        let s = p.snapshot();
        assert!(s.bytes_per_sec > 0.0);
        assert!(s.eta_secs.is_some_and(|e| e > 0.0));
        p.set_phase(Phase::Done);
        assert_eq!(p.snapshot().eta_secs, None);
    }
}
