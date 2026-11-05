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
pub fn recent_runs(paths: &Paths, id: &ProfileId, n: usize) -> Vec<RunRecord> {
    let path = profile_log_dir(paths, id).join("runs.jsonl");
    let Ok(f) = File::open(path) else {
        return Vec::new();
    };
    let mut v: Vec<RunRecord> = BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    v.reverse();
    v.truncate(n);
    v
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
        }
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
