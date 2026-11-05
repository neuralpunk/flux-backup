//! Live progress reporting shared by engines, the CLI and the GUI.
//!
//! Engines update lock-free counters in [`Progress`]. Front-ends poll [`Progress::snapshot`]
//! at whatever rate suits them, so the copying hot path never blocks on a slow consumer.
//! Rare events (phase changes, per-file warnings) go through a [`Reporter`] callback.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

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
}

impl Phase {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Phase::Scanning,
            2 => Phase::Planning,
            3 => Phase::Transferring,
            4 => Phase::Finalizing,
            5 => Phase::Done,
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
    /// Average copy rate during the transfer phase, bytes/second.
    pub bytes_per_sec: f64,
    /// Estimated seconds remaining in the transfer phase.
    pub eta_secs: Option<f64>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_phase(&self, p: Phase) {
        self.phase.store(p as u8, Relaxed);
        if let Ok(mut t) = self.phase_started.lock() {
            *t = Instant::now();
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
            let rate = bytes_done as f64 / phase_secs;
            (
                rate,
                Some(bytes_total.saturating_sub(bytes_done) as f64 / rate),
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
