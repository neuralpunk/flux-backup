//! Restore tests: now and then, restore a sample of a snapshot into a private temporary
//! folder and check every restored file against the backup's own record (and, if asked,
//! against the file on disk now). A backup that was written isn't proof that it can be read
//! back; this is.
//!
//! Modes: some random files (10 by default), a percentage of the size, everything, or
//! *rolling*: every file once per window (30 days by default). Rolling keeps one number,
//! a position in the files' hash order, and each test moves it on by the share of the
//! window that has passed since the last test, testing the files in between.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::profile::{Profile, RestoreTest, TestEvery, TestMode};
use flux_core::progress::{Progress, Reporter};
use flux_core::{Paths, ProfileId};
use flux_fs::entry::{Entry, Kind};
use flux_fs::restore::RestoreRequest;
use flux_fs::snapshot::SnapshotMeta;

use crate::repo::{AnyRepo, RepoError};

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestResult {
    Passed,
    /// It completed, but something deserves a look: files differing from the source now,
    /// permissions or times not as recorded, or it stopped at a limit.
    Warning,
    Failed,
}

/// What was wrong with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The restore of it failed (unreadable or damaged data).
    CouldNotRestore,
    SizeDiffers,
    /// Its checksum isn't the one recorded.
    ContentDiffers,
    /// Permissions or modification time not as recorded (a warning).
    MetadataDiffers,
    /// Not the same as the file on disk now (a warning: the file may have changed since).
    DiffersFromSource,
}

impl Problem {
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Problem::CouldNotRestore | Problem::SizeDiffers | Problem::ContentDiffers
        )
    }

    pub fn words(self) -> &'static str {
        match self {
            Problem::CouldNotRestore => "couldn't be restored",
            Problem::SizeDiffers => "size differs",
            Problem::ContentDiffers => "content differs",
            Problem::MetadataDiffers => "permissions or time differ",
            Problem::DiffersFromSource => "differs from the file now",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileProblem {
    pub path: String,
    pub problem: Problem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A test's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestReport {
    pub started: Timestamp,
    pub finished: Timestamp,
    pub snapshot: String,
    pub mode: TestMode,
    pub result: TestResult,
    /// Files picked for the test, and those checked (fewer when it stopped early).
    pub picked: u64,
    pub tested: u64,
    pub bytes: u64,
    /// A limit (time or data) stopped it before every picked file was checked.
    pub stopped_early: bool,
    /// At most 100.
    pub problems: Vec<FileProblem>,
    pub problems_total: u64,
    /// When the test itself couldn't run (no snapshot, the drive missing…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What's remembered between tests.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestState {
    pub last_passed: Option<Timestamp>,
    pub last_tested: Option<Timestamp>,
    /// Rolling mode: where the next test starts, in hash order.
    #[serde(default)]
    pub cursor: u64,
    /// The latest reports, newest last (at most 30).
    #[serde(default)]
    pub history: Vec<TestReport>,
}

const KEEP_REPORTS: usize = 30;
const KEEP_PROBLEMS: usize = 100;

fn state_path(paths: &Paths, id: &ProfileId) -> PathBuf {
    paths
        .state_dir
        .join("restore-tests")
        .join(format!("{id}.json"))
}

pub fn load_state(paths: &Paths, id: &ProfileId) -> TestState {
    std::fs::read(state_path(paths, id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_state(paths: &Paths, id: &ProfileId, s: &TestState) {
    let path = state_path(paths, id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(s) {
        let _ = flux_core::store::write_atomic(&path, &json, 0o600);
    }
}

/// How long a test may wait: the time between tests for each setting.
pub fn interval(every: TestEvery) -> Option<Duration> {
    const DAY: u64 = 86_400;
    match every {
        TestEvery::Off => None,
        TestEvery::Backup => Some(Duration::ZERO),
        TestEvery::Week => Some(Duration::from_secs(7 * DAY)),
        TestEvery::Month => Some(Duration::from_secs(30 * DAY)),
    }
}

/// Is a test due after a backup now?
pub fn due(paths: &Paths, id: &ProfileId, t: &RestoreTest, now: Timestamp) -> bool {
    let Some(every) = interval(t.every) else {
        return false;
    };
    match load_state(paths, id).last_passed {
        None => true,
        Some(p) => now.duration_since(p).as_secs() >= every.as_secs() as i64,
    }
}

/// When a passing test is overdue (shown as a warning): the interval and a grace period
/// (a week for "after every backup").
pub fn overdue(
    t: &RestoreTest,
    state: &TestState,
    oldest_snapshot: Option<Timestamp>,
    now: Timestamp,
) -> Option<i64> {
    let every = interval(t.every)?;
    let grace = match t.every {
        TestEvery::Backup => Duration::from_secs(7 * 86_400),
        _ => every + every / 4,
    };
    let since = state.last_passed.or(oldest_snapshot)?;
    let days = now.duration_since(since).as_secs() / 86_400;
    (now.duration_since(since).as_secs() > grace.as_secs() as i64).then_some(days)
}

/// A 64-bit position for a path in rolling order (FNV-1a: the same in every version, so
/// the rolling order survives updates).
fn position(path: &[u8]) -> u64 {
    path.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A private temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(base: &Path, name: &str) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::create_dir_all(base)?;
        let dir = base.join(format!(
            "{name}-{}-{}",
            std::process::id(),
            Timestamp::now().as_millisecond()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = flux_fs::fsops::remove_force(&self.0);
    }
}

/// Pick the files to test from a snapshot's files.
fn pick(files: Vec<Entry>, t: &RestoreTest, state: &mut TestState, now: Timestamp) -> Vec<Entry> {
    use rand_like::shuffle;
    match t.mode {
        TestMode::RandomFiles => {
            let mut f = files;
            shuffle(&mut f, now.as_nanosecond() as u64);
            f.truncate(t.files.max(1) as usize);
            f
        }
        TestMode::Percent => {
            let total: u64 = files.iter().map(|e| e.size).sum();
            let want = total * u64::from(t.percent.clamp(1, 100)) / 100;
            let mut f = files;
            shuffle(&mut f, now.as_nanosecond() as u64);
            let mut got = 0u64;
            let mut out = Vec::new();
            for e in f {
                if got >= want && !out.is_empty() {
                    break;
                }
                got += e.size;
                out.push(e);
            }
            out
        }
        TestMode::Full => files,
        TestMode::Rolling => {
            let window = t
                .window
                .map_or(Duration::from_secs(30 * 86_400), |w| w.as_duration());
            let elapsed = state.last_tested.map_or(window, |l| {
                Duration::from_secs(now.duration_since(l).as_secs().max(0) as u64)
            });
            let share = (elapsed.as_secs_f64() / window.as_secs_f64().max(1.0)).min(1.0);
            let span = (share * u64::MAX as f64) as u64;
            let (from, to) = (state.cursor, state.cursor.wrapping_add(span));
            state.cursor = to;
            files
                .into_iter()
                .filter(|e| {
                    let p = position(&e.path);
                    if share >= 1.0 {
                        true
                    } else if from <= to {
                        p >= from && p < to
                    } else {
                        p >= from || p < to
                    }
                })
                .collect()
        }
    }
}

/// Tiny deterministic shuffle (no dependency for a sample).
mod rand_like {
    pub fn shuffle<T>(v: &mut [T], seed: u64) {
        let mut x = seed | 1;
        for i in (1..v.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.swap(i, (x % (i as u64 + 1)) as usize);
        }
    }
}

/// Run a restore test of the newest snapshot. `t`: what to test (the profile's settings,
/// or other ones for a test by hand). Saves the report.
pub fn run(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    t: &RestoreTest,
    repo: &AnyRepo,
    progress: &Progress,
    report: Reporter<'_>,
) -> TestReport {
    let started = Timestamp::now();
    let mut state = load_state(paths, id);
    let rep = match run_inner(
        paths, id, profile, t, repo, &mut state, started, progress, report,
    ) {
        Ok(r) => r,
        Err(e) => TestReport {
            started,
            finished: Timestamp::now(),
            snapshot: String::new(),
            mode: t.mode,
            result: TestResult::Failed,
            picked: 0,
            tested: 0,
            bytes: 0,
            stopped_early: false,
            problems: Vec::new(),
            problems_total: 0,
            error: Some(e.to_string()),
        },
    };
    state.last_tested = Some(rep.started);
    if rep.result != TestResult::Failed {
        state.last_passed = Some(rep.started);
    }
    state.history.push(rep.clone());
    let n = state.history.len();
    if n > KEEP_REPORTS {
        state.history.drain(..n - KEEP_REPORTS);
    }
    save_state(paths, id, &state);
    rep
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    paths: &Paths,
    id: &ProfileId,
    profile: &Profile,
    t: &RestoreTest,
    repo: &AnyRepo,
    state: &mut TestState,
    started: Timestamp,
    progress: &Progress,
    report: Reporter<'_>,
) -> Result<TestReport, String> {
    let snap: SnapshotMeta = repo
        .snapshots()
        .map_err(|e| e.to_string())?
        .pop()
        .ok_or_else(|| RepoError::Empty(id.to_string()).to_string())?;
    let files: Vec<Entry> = repo
        .entries(&snap)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| e.kind == Kind::File)
        .collect();
    let picked = pick(files, t, state, started);
    let tmp = TempDir::new(&paths.cache_dir.join("restore-tests"), id.as_str())
        .map_err(|e| format!("can't make a temporary folder: {e}"))?;
    let deadline = t.max_runtime.map(|d| Instant::now() + d.as_duration());
    let max_bytes = t.max_read.map(|b| b.bytes());
    let xattrs = flux_fs::scan::xattr_policy(profile);

    // Batches whose files all have different names (each lands at <batch>/<its name>),
    // small enough that limits are checked often.
    let per_batch = if deadline.is_some() || max_bytes.is_some() {
        5
    } else {
        25
    };
    let mut batches: Vec<Vec<&Entry>> = Vec::new();
    for e in &picked {
        let name = Path::new(std::ffi::OsStr::from_bytes(&e.path)).file_name();
        match batches.iter_mut().find(|b| {
            !b.iter()
                .any(|x| Path::new(std::ffi::OsStr::from_bytes(&x.path)).file_name() == name)
                && b.len() < per_batch
        }) {
            Some(b) => b.push(e),
            None => batches.push(vec![e]),
        }
    }

    let mut problems: Vec<FileProblem> = Vec::new();
    let mut tested = 0u64;
    let mut bytes = 0u64;
    let mut stopped_early = false;
    for (k, batch) in batches.iter().enumerate() {
        if progress.is_cancelled()
            || deadline.is_some_and(|d| Instant::now() > d)
            || max_bytes.is_some_and(|m| bytes >= m)
        {
            stopped_early = true;
            break;
        }
        let to = tmp.path().join(format!("b{k}"));
        let req = RestoreRequest {
            snapshot: &snap,
            paths: batch
                .iter()
                .map(|e| PathBuf::from("/").join(e.rel_path()))
                .collect(),
            to: Some(to.clone()),
            keep_existing: true,
            dry_run: false,
            xattrs,
            links: flux_fs::safe::LinkPolicy::for_process(),
            exact: None,
        };
        let failed: HashMap<String, String> = match repo.restore(&req, progress, report) {
            Ok(out) => out
                .errors
                .into_iter()
                .map(|e| (e.path, e.message))
                .collect(),
            Err(e) if e.is_cancelled() => {
                stopped_early = true;
                break;
            }
            Err(e) => batch
                .iter()
                .map(|x| (format!("/{}", x.rel_path().display()), e.to_string()))
                .collect(),
        };
        for e in batch {
            let name = e.rel_path().file_name().unwrap_or_default().to_owned();
            let restored = to.join(&name);
            let shown = format!("/{}", e.rel_path().display());
            tested += 1;
            bytes += e.size;
            let mut add = |problem: Problem, detail: Option<String>| {
                problems.push(FileProblem {
                    path: shown.clone(),
                    problem,
                    detail,
                })
            };
            let Ok(m) = std::fs::symlink_metadata(&restored) else {
                let why = failed
                    .iter()
                    .find(|(p, _)| p.ends_with(&*name.to_string_lossy()))
                    .map(|(_, m)| m.clone());
                add(Problem::CouldNotRestore, why);
                continue;
            };
            if m.len() != e.size {
                add(
                    Problem::SizeDiffers,
                    Some(format!("{} bytes, recorded {}", m.len(), e.size)),
                );
                continue;
            }
            if let Some(h) = e.hash
                && flux_fs::fsops::hash_file(&restored).ok() != Some(h)
            {
                add(Problem::ContentDiffers, None);
                continue;
            }
            if m.mode() & 0o777 != e.mode & 0o777 || m.mtime() != e.mtime {
                add(Problem::MetadataDiffers, None);
            }
            if t.compare_with_source {
                let source = PathBuf::from("/").join(e.rel_path());
                if let Ok(sm) = std::fs::metadata(&source) {
                    let same_stamp = sm.len() == e.size && sm.mtime() == e.mtime;
                    let differs = if same_stamp {
                        // Same size and time: the bytes must match too.
                        flux_fs::fsops::hash_file(&source).ok()
                            != flux_fs::fsops::hash_file(&restored).ok()
                    } else {
                        true
                    };
                    if differs {
                        add(
                            Problem::DiffersFromSource,
                            Some(if same_stamp {
                                "the same size and time, but different bytes".into()
                            } else {
                                "changed since this snapshot".into()
                            }),
                        );
                    }
                }
            }
        }
        // Free the space as soon as a batch is checked.
        let _ = flux_fs::fsops::remove_force(&to);
    }
    let failed = problems.iter().any(|p| p.problem.is_failure());
    let result = if failed {
        TestResult::Failed
    } else if !problems.is_empty() || stopped_early {
        TestResult::Warning
    } else {
        TestResult::Passed
    };
    let problems_total = problems.len() as u64;
    problems.sort_by_key(|p| !p.problem.is_failure());
    problems.truncate(KEEP_PROBLEMS);
    Ok(TestReport {
        started,
        finished: Timestamp::now(),
        snapshot: snap.id,
        mode: t.mode,
        result,
        picked: picked.len() as u64,
        tested,
        bytes,
        stopped_early,
        problems,
        problems_total,
        error: None,
    })
}

/// "Passed: 10 files, 4.2 MiB" (for logs and notifications).
pub fn summary(r: &TestReport) -> String {
    if let Some(e) = &r.error {
        return format!("restore test couldn't run: {e}");
    }
    let what = format!(
        "{} files, {}{}",
        r.tested,
        flux_core::ByteSize(r.bytes).display_approx(),
        if r.stopped_early {
            format!(" (stopped at a limit; {} picked)", r.picked)
        } else {
            String::new()
        }
    );
    match r.result {
        TestResult::Passed => format!("restore test passed: {what}"),
        TestResult::Warning => format!(
            "restore test passed with {} notes: {what}",
            r.problems_total
        ),
        TestResult::Failed => format!(
            "restore test FAILED: {} of {} files have problems ({})",
            r.problems.iter().filter(|p| p.problem.is_failure()).count(),
            r.tested,
            r.problems
                .iter()
                .find(|p| p.problem.is_failure())
                .map(|p| format!("{}: {}", p.path, p.problem.words()))
                .unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> Entry {
        Entry {
            path: path.as_bytes().to_vec(),
            kind: Kind::File,
            mode: 0o644,
            uid: 0,
            gid: 0,
            size,
            mtime: 0,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            nlink: 1,
            rdev: 0,
            link: None,
            hash: None,
            xattrs: Vec::new(),
        }
    }

    #[test]
    fn picking() {
        let files: Vec<Entry> = (0..200).map(|i| entry(&format!("f{i}"), 100)).collect();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let mut st = TestState::default();
        let t = |mode| RestoreTest {
            mode,
            ..RestoreTest::default()
        };
        assert_eq!(
            pick(files.clone(), &t(TestMode::RandomFiles), &mut st, now).len(),
            10
        );
        let pct = RestoreTest {
            percent: 5,
            ..t(TestMode::Percent)
        };
        assert_eq!(
            pick(files.clone(), &pct, &mut st, now).len(),
            10,
            "5% of 200 equal files"
        );
        assert_eq!(
            pick(files.clone(), &t(TestMode::Full), &mut st, now).len(),
            200
        );

        // Rolling over 30 days, tested every 10 days: three tests cover every file once.
        let roll = t(TestMode::Rolling);
        let mut st = TestState {
            last_tested: Some(now - jiff::SignedDuration::from_hours(240)),
            ..TestState::default()
        };
        let mut seen = std::collections::HashSet::new();
        let mut count = 0;
        for k in 0..3 {
            let at = now + jiff::SignedDuration::from_hours(240 * k);
            for e in pick(files.clone(), &roll, &mut st, at) {
                count += 1;
                seen.insert(e.path);
            }
            st.last_tested = Some(at);
        }
        assert_eq!(seen.len(), 200, "every file once");
        assert_eq!(count, 200, "and only once");
    }

    #[test]
    fn overdue_after_the_interval_and_grace() {
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let days = |d: i64| now - jiff::SignedDuration::from_hours(24 * d);
        let monthly = RestoreTest {
            every: TestEvery::Month,
            ..RestoreTest::default()
        };
        let passed = |d| TestState {
            last_passed: Some(days(d)),
            ..TestState::default()
        };
        assert_eq!(overdue(&monthly, &passed(20), None, now), None);
        assert_eq!(overdue(&monthly, &passed(40), None, now), Some(40));
        assert_eq!(
            overdue(&monthly, &TestState::default(), Some(days(50)), now),
            Some(50),
            "never passed"
        );
        assert_eq!(
            overdue(&RestoreTest::default(), &passed(400), None, now),
            None,
            "off"
        );
    }
}
