//! Per-run logs under `<state>/logs/<profile>/`:
//!
//! * `runs.jsonl`: one [`RunRecord`] per line, the run history (oldest first)
//! * `<run-id>.jsonl`: timestamped [`Event`]s of one run (warnings, hook output, …)

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::hooks::HookRun;
use flux_core::progress::Event;
use flux_core::{Paths, ProfileId};
use flux_mirror::repo::SnapshotStats;

/// Event files kept per profile.
const KEEP_EVENT_FILES: usize = 50;
/// History lines kept per profile (trimmed when exceeded by 20%).
const KEEP_RECORDS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Scheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    CompletedWithErrors,
    Unchanged,
    DryRun,
    Skipped,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Completed => "completed",
            RunStatus::CompletedWithErrors => "completed with errors",
            RunStatus::Unchanged => "no changes",
            RunStatus::DryRun => "dry run",
            RunStatus::Skipped => "skipped",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Unchanged | RunStatus::DryRun
        )
    }
}

/// Summary of one run, as stored in `runs.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub profile: String,
    pub trigger: Trigger,
    pub started: Timestamp,
    pub finished: Timestamp,
    pub status: RunStatus,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub pruned: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub stats: Option<SnapshotStats>,
    #[serde(default)]
    pub warnings: u64,
    #[serde(default)]
    pub hooks: Vec<HookRun>,
    /// Free space where the backups go, after the run (local targets): the history the
    /// space forecast is made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_after: Option<u64>,
    /// Sources whose drive wasn't connected (skipped: kept from before, or left out).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources_missing: Vec<std::path::PathBuf>,
    /// How the backup went at each of the profile's other destinations (the fields above
    /// are the main destination's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<DestinationRun>,
    /// Unusual changes found in this backup (see [`crate::unusual`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unusual: Option<crate::unusual::UnusualReport>,
    /// Where the time went (the performance inspector).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance: Option<Performance>,
}

/// Where a run's time went.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Performance {
    /// Wall-clock seconds of each phase (scanning, comparing, copying…), in order.
    pub phases: Vec<Timed>,
    /// Seconds per stage of the work, summed over threads (so they can add up to more
    /// than the run took): reading, chunking, finding what's stored, compressing and
    /// encrypting, writing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<Timed>,
    /// Writes to the storage, their bytes, and the average time one took.
    #[serde(default)]
    pub writes: u64,
    #[serde(default)]
    pub bytes_written: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_ms: Option<f64>,
    /// Bytes of files processed per second while copying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throughput: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timed {
    pub name: String,
    pub secs: f64,
}

impl Performance {
    /// From a run's progress counters.
    pub fn of(progress: &flux_core::progress::Progress) -> Option<Self> {
        let t = &progress.timings;
        let phases: Vec<Timed> = t
            .phases()
            .into_iter()
            .map(|(name, secs)| Timed { name, secs })
            .collect();
        if phases.is_empty() {
            return None;
        }
        let writes = t.writes.load(std::sync::atomic::Ordering::Relaxed);
        let stages: Vec<(flux_core::progress::Stage, f64)> = t.stages();
        let write_secs = stages
            .iter()
            .find(|(s, _)| *s == flux_core::progress::Stage::Write)
            .map(|(_, s)| *s);
        let copying = phases
            .iter()
            .find(|p| p.name == flux_core::progress::Phase::Transferring.label())
            .map(|p| p.secs)
            .filter(|s| *s > 0.0);
        let done = progress
            .bytes_done
            .load(std::sync::atomic::Ordering::Relaxed);
        Some(Performance {
            stages: stages
                .into_iter()
                .map(|(s, secs)| Timed {
                    name: s.label().into(),
                    secs,
                })
                .collect(),
            phases,
            writes,
            bytes_written: t.bytes_written.load(std::sync::atomic::Ordering::Relaxed),
            write_ms: write_secs
                .filter(|_| writes > 0)
                .map(|s| s * 1000.0 / writes as f64),
            throughput: copying.filter(|_| done > 0).map(|s| done as f64 / s),
        })
    }
}

/// One destination's part of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DestinationRun {
    /// The destination's name in the profile (`nas`).
    pub name: String,
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<SnapshotStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedEvent {
    pub t: Timestamp,
    #[serde(flatten)]
    pub event: Event,
}

pub fn profile_log_dir(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths.logs_dir().join(id.as_str())
}

/// An open run log. Writing is best-effort: logging problems never fail a backup.
pub struct RunLog {
    pub run_id: String,
    dir: PathBuf,
    events: Mutex<Option<BufWriter<File>>>,
}

impl RunLog {
    pub fn start(paths: &Paths, id: &ProfileId, started: Timestamp) -> Self {
        let dir = profile_log_dir(paths, id);
        let zoned = started.to_zoned(jiff::tz::TimeZone::system());
        let base = format!(
            "{}-{:03}",
            zoned.strftime("%Y%m%d-%H%M%S"),
            zoned.subsec_nanosecond() / 1_000_000
        );
        let _ = flux_core::store::create_private_dir(&dir);
        // Unique even for several runs within one millisecond; create_new avoids races.
        let mut run_id = base.clone();
        let mut file = None;
        for n in 1..100 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(format!("{run_id}.jsonl")))
            {
                Ok(f) => {
                    file = Some(BufWriter::new(f));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    run_id = format!("{base}-{}", n + 1)
                }
                Err(_) => break,
            }
        }
        if file.is_none() {
            tracing::warn!("cannot write run log in {}", dir.display());
        }
        Self {
            run_id,
            dir,
            events: Mutex::new(file),
        }
    }

    pub fn event(&self, event: &Event) {
        if let Ok(mut guard) = self.events.lock()
            && let Some(w) = guard.as_mut()
        {
            let line = LoggedEvent {
                t: Timestamp::now(),
                event: event.clone(),
            };
            if serde_json::to_writer(&mut *w, &line).is_ok() {
                let _ = w.write_all(b"\n");
            }
        }
    }

    /// Close the event file and append `record` to the history.
    pub fn finish(self, record: &RunRecord) {
        if let Ok(mut g) = self.events.lock()
            && let Some(mut w) = g.take()
        {
            let _ = w.flush();
        }
        if let Err(e) = append_record(&self.dir, record) {
            tracing::warn!("cannot record run in {}: {e}", self.dir.display());
        }
        rotate_event_files(&self.dir);
    }
}

fn append_record(dir: &Path, record: &RunRecord) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join("runs.jsonl");
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    f.write_all(&line)?;
    drop(f);
    let lines: Vec<String> = BufReader::new(File::open(&path)?)
        .lines()
        .collect::<Result<_, _>>()?;
    if lines.len() > KEEP_RECORDS + KEEP_RECORDS / 5 {
        let keep = lines[lines.len() - KEEP_RECORDS..].join("\n") + "\n";
        flux_core::store::write_atomic(&path, keep.as_bytes(), 0o600)
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

/// Remove history older than `days` and beyond the newest `runs` runs (Activity's
/// "keep history"), with the events of the runs removed. Snapshots aren't touched.
pub fn trim(paths: &Paths, id: &ProfileId, days: Option<u32>, runs: Option<u32>, now: Timestamp) {
    let dir = profile_log_dir(paths, id);
    let path = dir.join("runs.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let records: Vec<(&str, Option<RunRecord>)> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| (l, serde_json::from_str(l).ok()))
        .collect();
    let cutoff = days.map(|d| now - jiff::SignedDuration::from_hours(24 * i64::from(d)));
    let keep_from = runs.map_or(0, |n| records.len().saturating_sub(n as usize));
    let mut kept = String::new();
    let mut gone = Vec::new();
    for (i, (line, rec)) in records.iter().enumerate() {
        let old = cutoff.is_some_and(|c| rec.as_ref().is_some_and(|r| r.finished < c));
        if i < keep_from || old {
            if let Some(r) = rec
                && !r.run_id.is_empty()
            {
                gone.push(r.run_id.clone());
            }
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if gone.is_empty() {
        return;
    }
    if flux_core::store::write_atomic(&path, kept.as_bytes(), 0o600).is_ok() {
        for run in gone {
            let _ = fs::remove_file(dir.join(format!("{run}.jsonl")));
        }
    }
}

fn rotate_event_files(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "jsonl")
                && p.file_name().is_some_and(|n| n != "runs.jsonl")
        })
        .collect();
    if files.len() <= KEEP_EVENT_FILES {
        return;
    }
    files.sort(); // run ids start with a sortable timestamp
    for old in &files[..files.len() - KEEP_EVENT_FILES] {
        let _ = fs::remove_file(old);
    }
}

/// The last `n` runs of a profile, newest first.
///
/// Reads the history backwards from its end and stops after `n` records, so asking for the
/// latest run (the sidebar does, for every backup) costs one small read, not a parse of the
/// whole file.
pub fn recent_runs(paths: &Paths, id: &ProfileId, n: usize) -> Vec<RunRecord> {
    let path = profile_log_dir(paths, id).join("runs.jsonl");
    let Ok(f) = File::open(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in TailLines::new(f) {
        if out.len() >= n {
            break;
        }
        if let Ok(r) = serde_json::from_slice(&line) {
            out.push(r);
        }
    }
    out
}

/// The lines of a file, last first, read in blocks from the end.
struct TailLines {
    file: File,
    /// Bytes not read yet: `0..pos`.
    pos: u64,
    /// A partial line carried over from the block read before (it continues the next one).
    carry: Vec<u8>,
    ready: Vec<Vec<u8>>,
}

impl TailLines {
    const BLOCK: u64 = 64 * 1024;

    fn new(file: File) -> Self {
        let pos = file.metadata().map_or(0, |m| m.len());
        Self {
            file,
            pos,
            carry: Vec::new(),
            ready: Vec::new(),
        }
    }
}

impl Iterator for TailLines {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        loop {
            if let Some(l) = self.ready.pop() {
                return Some(l);
            }
            if self.pos == 0 {
                let last = std::mem::take(&mut self.carry);
                return (!last.is_empty()).then_some(last);
            }
            let len = Self::BLOCK.min(self.pos);
            self.pos -= len;
            let mut block = vec![0u8; len as usize];
            if self.file.read_exact_at(&mut block, self.pos).is_err() {
                self.pos = 0;
                self.carry.clear();
                return None;
            }
            block.extend_from_slice(&std::mem::take(&mut self.carry));
            let mut parts = block.split(|&b| b == b'\n');
            // The first part may continue in the block before this one.
            self.carry = parts.next().unwrap_or_default().to_vec();
            // Oldest first here; `ready` pops the newest.
            self.ready = parts
                .filter(|l| !l.is_empty())
                .map(<[u8]>::to_vec)
                .collect();
        }
    }
}

/// The events of one run (empty if its file was rotated away).
pub fn run_events(paths: &Paths, id: &ProfileId, run_id: &str) -> Vec<LoggedEvent> {
    if run_id.contains('/') {
        return Vec::new();
    }
    let path = profile_log_dir(paths, id).join(format!("{run_id}.jsonl"));
    let Ok(f) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

#[cfg(test)]
mod tail_tests {
    use super::*;

    #[test]
    fn tail_lines_cross_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        // Lines longer than a block, and a file without a final newline.
        let long = "x".repeat(150_000);
        std::fs::write(&p, format!("one\n{long}\n\nthree\nfour")).unwrap();
        let lines: Vec<String> = TailLines::new(File::open(&p).unwrap())
            .map(|l| String::from_utf8(l).unwrap())
            .collect();
        assert_eq!(
            lines,
            vec!["four".to_owned(), "three".into(), long, "one".into()]
        );
        std::fs::write(&p, "").unwrap();
        assert_eq!(TailLines::new(File::open(&p).unwrap()).count(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: run_id.into(),
            profile: "p".into(),
            trigger: Trigger::Manual,
            started: Timestamp::now(),
            finished: Timestamp::now(),
            status: RunStatus::Completed,
            snapshot: Some("s".into()),
            pruned: vec![],
            error: None,
            stats: None,
            warnings: 0,
            hooks: vec![],
            free_after: None,
            sources_missing: Vec::new(),
            destinations: Vec::new(),
            unusual: None,
            performance: None,
        }
    }

    #[test]
    fn trim_by_age_and_count() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        let id = ProfileId::new("p").unwrap();
        let dir = profile_log_dir(&paths, &id);
        let now = Timestamp::now();
        for (n, days_ago) in [(1, 40), (2, 20), (3, 5), (4, 1)] {
            let mut r = record(&format!("run{n}"));
            r.finished = now - jiff::SignedDuration::from_hours(24 * days_ago);
            append_record(&dir, &r).unwrap();
            fs::write(dir.join(format!("run{n}.jsonl")), "{}\n").unwrap();
        }
        trim(&paths, &id, Some(30), None, now);
        let ids: Vec<String> = recent_runs(&paths, &id, 10)
            .into_iter()
            .map(|r| r.run_id)
            .collect();
        assert_eq!(ids, ["run4", "run3", "run2"]);
        assert!(!dir.join("run1.jsonl").exists() && dir.join("run2.jsonl").exists());
        trim(&paths, &id, None, Some(1), now);
        let ids: Vec<String> = recent_runs(&paths, &id, 10)
            .into_iter()
            .map(|r| r.run_id)
            .collect();
        assert_eq!(ids, ["run4"]);
        assert!(!dir.join("run3.jsonl").exists() && dir.join("run4.jsonl").exists());
        trim(&paths, &id, Some(30), Some(5), now); // nothing more to remove
        assert_eq!(recent_runs(&paths, &id, 10).len(), 1);
    }

    #[test]
    fn write_and_read_back() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::under(t.path(), flux_core::Scope::User);
        let id = ProfileId::new("p").unwrap();
        let log = RunLog::start(&paths, &id, Timestamp::now());
        let run_id = log.run_id.clone();
        log.event(&Event::Warning {
            path: "/x".into(),
            message: "nope".into(),
        });
        log.finish(&record(&run_id));
        let runs = recent_runs(&paths, &id, 10);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, run_id);
        let ev = run_events(&paths, &id, &run_id);
        assert!(matches!(&ev[0].event, Event::Warning { path, .. } if path == "/x"));
    }

    #[test]
    fn rotation() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        for i in 0..(KEEP_EVENT_FILES + 5) {
            fs::write(dir.join(format!("2026{i:04}.jsonl")), "").unwrap();
        }
        fs::write(dir.join("runs.jsonl"), "").unwrap();
        rotate_event_files(dir);
        let n = fs::read_dir(dir).unwrap().count();
        assert_eq!(n, KEEP_EVENT_FILES + 1);
        assert!(dir.join("runs.jsonl").exists());
        assert!(!dir.join("20260000.jsonl").exists());
    }
}
