//! Restore tests: now and then, read a sample of a snapshot back the way a restore reads
//! it, and check every file against the backup's own record (and, if asked, against the
//! file on disk now). A backup that was written isn't proof that it can be read back; this
//! is.
//!
//! Nothing is restored anywhere. Each file streams out of the backup (decrypted and
//! decompressed, for a Vault) through a small buffer, into a checksum, and the buffer is
//! wiped after each piece: no copy lands on a disk or in a temporary folder, only a piece
//! of one file is in memory at a time, so an encrypted backup's files stay encrypted at
//! rest throughout, and memory use is the same for ten files as for every file. (Before
//! 1.1 the sample was restored into a temporary folder; one left behind by a crash is
//! removed.)
//!
//! Modes: some random files (10 by default), a percentage of the size, everything,
//! *rolling*: every file once per window (30 days by default), or *confidence*: as many
//! random files as it takes to catch damage to a given share of the files with a given
//! certainty (the arithmetic is in [`crate::sample`]). Rolling keeps one number, a
//! position in the files' hash order, and each test moves it on by the share of the
//! window that has passed since the last test, testing the files in between.
//!
//! A test of a uniform random sample that passes says what it would have caught: its
//! report carries a [`Guarantee`].

use std::collections::HashMap;
use std::io::{self, Read};
use std::ops::ControlFlow;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use flux_core::profile::{Profile, RestoreTest, TestEvery, TestMode};
use flux_core::progress::{Progress, Reporter};
use flux_core::{Paths, Percent, ProfileId};
use flux_fs::entry::{Entry, Kind};
use flux_fs::snapshot::SnapshotMeta;
use flux_vault::format::Content;
use zeroize::Zeroizing;

use crate::repo::{AnyRepo, RepoError};
use crate::sample;

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
    /// It couldn't be read back (unreadable or damaged data).
    CouldNotRestore,
    SizeDiffers,
    /// Its checksum isn't the one recorded.
    ContentDiffers,
    /// Permissions or modification time not as recorded (a warning). Only in reports from
    /// before 1.1, which restored files to a folder; nothing is restored now.
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

/// What a test that passed proves: had `damaged` or more of the snapshot's `files` (a
/// share `damage` or more) been damaged, it would have found at least one of them with
/// probability `certainty` or better. Only said of a uniform random sample (or every
/// file) read to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guarantee {
    /// Files in the snapshot (the sample was drawn from all of them).
    pub files: u64,
    pub damaged: u64,
    pub damage: Percent,
    pub certainty: Percent,
}

/// The certainty at which a plain random sample's worth is stated.
fn stated_certainty() -> Percent {
    Percent::from_millionths(990_000)
}

/// A share rounded up to a figure worth reading (37%, 4.6%, 0.05%). Up, so that "this
/// share or more" never claims more than the files counted.
fn round_up(p: Percent) -> Percent {
    let m = p.millionths();
    let step = match m {
        100_000.. => 10_000,
        10_000.. => 1_000,
        _ => 100,
    };
    Percent::from_millionths(m.div_ceil(step) * step)
}

/// What a passing test of `tested` files out of `files` proves (see [`Guarantee`]).
fn guarantee(t: &RestoreTest, files: u64, tested: u64) -> Option<Guarantee> {
    if files == 0 || tested == 0 {
        return None;
    }
    match t.mode {
        // The sample was sized for exactly this.
        TestMode::Confidence if tested >= sample::sample_size(files, t.damage, t.certainty) => {
            Some(Guarantee {
                files,
                damaged: sample::damaged_at_least(files, t.damage),
                damage: t.damage,
                certainty: t.certainty,
            })
        }
        // So many random files: what would they have caught, 99 times in 100?
        TestMode::RandomFiles => {
            let certainty = stated_certainty();
            let damaged = sample::detectable(files, tested, certainty)?;
            Some(Guarantee {
                files,
                damaged,
                damage: round_up(sample::share(damaged, files)),
                certainty,
            })
        }
        // Percent stops by size and rolling goes in hash order: neither is a uniform
        // sample of the files. Full has read every file, which needs no arithmetic.
        _ => None,
    }
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
    /// What passing proves, when the sample allows saying so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guarantee: Option<Guarantee>,
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

/// Pick the files to test from a snapshot's files.
fn pick(
    files: Vec<Entry>,
    t: &RestoreTest,
    state: &mut TestState,
    now: Timestamp,
    rng: &mut sample::Rng,
) -> Vec<Entry> {
    // A uniform random sample of `n` files, in random order.
    let take = |mut f: Vec<Entry>, n: usize, rng: &mut sample::Rng| {
        sample::sample_to_front(&mut f, n, rng);
        f.truncate(n);
        f
    };
    match t.mode {
        TestMode::RandomFiles => take(files, t.files.max(1) as usize, rng),
        TestMode::Confidence => {
            let n = sample::sample_size(files.len() as u64, t.damage, t.certainty);
            take(files, n as usize, rng)
        }
        TestMode::Percent => {
            let total: u64 = files.iter().map(|e| e.size).sum();
            let want = total * u64::from(t.percent.clamp(1, 100)) / 100;
            let mut f = files;
            let all = f.len();
            sample::sample_to_front(&mut f, all, rng);
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

/// Run a restore test of the newest snapshot. `t`: what to test (the profile's settings,
/// or other ones for a test by hand); `progress` can cancel it. Saves the report.
/// (`_profile` and `_report` are kept for callers; nothing is restored, so neither is
/// needed any more.)
pub fn run(
    paths: &Paths,
    id: &ProfileId,
    _profile: &Profile,
    t: &RestoreTest,
    repo: &AnyRepo,
    progress: &Progress,
    _report: Reporter<'_>,
) -> TestReport {
    let started = Timestamp::now();
    let mut state = load_state(paths, id);
    let rep = match run_inner(paths, id, t, repo, &mut state, started, progress) {
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
            guarantee: None,
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

/// How much of a file is read at a time: the most of it that's ever in memory at once
/// (besides one Vault chunk being decoded).
const PIECE: usize = 1 << 20;

/// Read file `i` of a snapshot from start to end, the way a restore reads it, handing each
/// piece to `f` (which may stop it early). Nothing is written anywhere: each piece is
/// wiped once `f` has seen it.
fn read_through(
    repo: &AnyRepo,
    snap: &SnapshotMeta,
    entries: &[Entry],
    blobs: &[Content],
    i: usize,
    f: &mut dyn FnMut(&[u8]) -> ControlFlow<()>,
) -> io::Result<()> {
    if let AnyRepo::Borg(r) = repo {
        // Borg's own reader for one file extracts it to a temporary file: stream instead.
        let mut stream = r.stream_file(&snap.id, &entries[i])?;
        let mut buf = Zeroizing::new(vec![0u8; PIECE]);
        loop {
            let n = match stream.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if n == 0 || f(&buf[..n]).is_break() {
                return Ok(());
            }
        }
    }
    let mut file = crate::view::open_file(repo, snap, entries, blobs, i)?;
    let mut offset = 0u64;
    loop {
        let piece = Zeroizing::new(file.read_at(offset, PIECE)?);
        if piece.is_empty() || f(&piece).is_break() {
            return Ok(());
        }
        offset += piece.len() as u64;
    }
}

fn run_inner(
    paths: &Paths,
    id: &ProfileId,
    t: &RestoreTest,
    repo: &AnyRepo,
    state: &mut TestState,
    started: Timestamp,
    progress: &Progress,
) -> Result<TestReport, String> {
    // Restore tests before 1.1 restored their sample here; one a crash left holds plain
    // copies of backed-up files.
    let old = paths.cache_dir.join("restore-tests");
    if old.exists() {
        let _ = flux_fs::fsops::remove_force(&old);
    }
    let snap: SnapshotMeta = repo
        .snapshots()
        .map_err(|e| e.to_string())?
        .pop()
        .ok_or_else(|| RepoError::Empty(id.to_string()).to_string())?;
    // Reading a Vault: hold its shared lock, so upkeep can't remove data being read.
    let _lock = match repo.vault() {
        Some(v) => Some(v.lock(false).map_err(|e| e.to_string())?),
        None => None,
    };
    let (entries, blobs) = crate::view::load_entries(repo, &snap).map_err(|e| e.to_string())?;
    let files: Vec<Entry> = entries
        .iter()
        .filter(|e| e.kind == Kind::File)
        .cloned()
        .collect();
    let files_total = files.len() as u64;
    let picked = pick(files, t, state, started, &mut sample::Rng::from_os());
    let at: HashMap<&[u8], usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.path.as_slice(), i))
        .collect();
    let deadline = t.max_runtime.map(|d| Instant::now() + d.as_duration());
    let max_bytes = t.max_read.map(|b| b.bytes());
    let over_time = || progress.is_cancelled() || deadline.is_some_and(|d| Instant::now() > d);

    let mut problems: Vec<FileProblem> = Vec::new();
    let mut tested = 0u64;
    let mut bytes = 0u64;
    let mut stopped_early = false;
    for e in &picked {
        if over_time() || max_bytes.is_some_and(|m| bytes >= m) {
            stopped_early = true;
            break;
        }
        let Some(&i) = at.get(e.path.as_slice()) else {
            continue;
        };
        let mut hasher = blake3::Hasher::new();
        let mut read = 0u64;
        let mut stopped = false;
        let outcome = read_through(repo, &snap, &entries, &blobs, i, &mut |piece| {
            hasher.update(piece);
            read += piece.len() as u64;
            if over_time() {
                stopped = true;
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        if stopped {
            // Cut short by a limit: this file wasn't checked, so it isn't counted.
            stopped_early = true;
            break;
        }
        tested += 1;
        bytes += e.size;
        let shown = format!("/{}", e.rel_path().display());
        let mut add = |problem: Problem, detail: Option<String>| {
            problems.push(FileProblem {
                path: shown.clone(),
                problem,
                detail,
            })
        };
        if let Err(err) = outcome {
            add(Problem::CouldNotRestore, Some(err.to_string()));
            continue;
        }
        if read != e.size {
            add(
                Problem::SizeDiffers,
                Some(format!("{read} bytes, recorded {}", e.size)),
            );
            continue;
        }
        let got = *hasher.finalize().as_bytes();
        if e.hash.is_some_and(|h| h != got) {
            add(Problem::ContentDiffers, None);
            continue;
        }
        if t.compare_with_source {
            let source = PathBuf::from("/").join(e.rel_path());
            // Only a regular file there now (never through a link): it's compared by its
            // checksum, and only "differs" is said about it.
            if let Ok(sm) = std::fs::symlink_metadata(&source)
                && sm.file_type().is_file()
            {
                let same_stamp = sm.len() == e.size && sm.mtime() == e.mtime;
                let differs = if same_stamp {
                    // Same size and time: the bytes must match too.
                    flux_fs::fsops::hash_file(&source).ok() != Some(got)
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
    let failed = problems.iter().any(|p| p.problem.is_failure());
    let result = if failed {
        TestResult::Failed
    } else if !problems.is_empty() || stopped_early {
        TestResult::Warning
    } else {
        TestResult::Passed
    };
    // What passing proves: only said when nothing was wrong with the backup and the whole
    // sample was read (one cut short by a limit is no longer a uniform sample: big files
    // end it sooner).
    let guarantee = (!failed && !stopped_early && tested == picked.len() as u64)
        .then(|| guarantee(t, files_total, tested))
        .flatten();
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
        guarantee,
    })
}

/// "if 1% or more of its 120,000 files were damaged, this test would have found it (99%
/// certain)": a guarantee, in words.
pub fn guarantee_words(g: &Guarantee) -> String {
    let files = group(g.files);
    if g.damaged <= 1 {
        return format!(
            "if even one of its {files} files were damaged, this test would have found it ({} certain)",
            g.certainty
        );
    }
    format!(
        "if {} or more of its {files} files were damaged, this test would have found it ({} certain)",
        g.damage, g.certainty
    )
}

/// 120000 as "120,000".
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "Passed: 10 files, 4.2 MiB", and what that proves (for logs and notifications).
pub fn summary(r: &TestReport) -> String {
    let proves = r
        .guarantee
        .as_ref()
        .map(|g| format!(": {}", guarantee_words(g)))
        .unwrap_or_default();
    format!("{}{proves}", headline(r))
}

/// "Passed: 10 files, 4.2 MiB".
pub fn headline(r: &TestReport) -> String {
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
        let rng = &mut sample::Rng::seeded(1);
        let t = |mode| RestoreTest {
            mode,
            ..RestoreTest::default()
        };
        assert_eq!(
            pick(files.clone(), &t(TestMode::RandomFiles), &mut st, now, rng).len(),
            10
        );
        let pct = RestoreTest {
            percent: 5,
            ..t(TestMode::Percent)
        };
        assert_eq!(
            pick(files.clone(), &pct, &mut st, now, rng).len(),
            10,
            "5% of 200 equal files"
        );
        assert_eq!(
            pick(files.clone(), &t(TestMode::Full), &mut st, now, rng).len(),
            200
        );

        // Confidence: 1% at 99% of 200 files is 2 damaged files or more, which takes 180.
        let sure = pick(files.clone(), &t(TestMode::Confidence), &mut st, now, rng);
        assert_eq!(sure.len(), 180);
        let distinct: std::collections::HashSet<_> = sure.iter().map(|e| &e.path).collect();
        assert_eq!(distinct.len(), 180, "no file twice");

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
            for e in pick(files.clone(), &roll, &mut st, at, rng) {
                count += 1;
                seen.insert(e.path);
            }
            st.last_tested = Some(at);
        }
        assert_eq!(seen.len(), 200, "every file once");
        assert_eq!(count, 200, "and only once");
    }

    #[test]
    fn what_passing_proves() {
        let t = |mode| RestoreTest {
            mode,
            ..RestoreTest::default()
        };
        // Confidence, the whole sample read: what was asked for.
        let g = guarantee(&t(TestMode::Confidence), 120_000, 459).unwrap();
        assert_eq!((g.damaged, g.files), (1_200, 120_000));
        assert_eq!(
            guarantee_words(&g),
            "if 1% or more of its 120,000 files were damaged, this test would have found it (99% certain)"
        );
        // Too few files for it: nothing is claimed.
        assert_eq!(guarantee(&t(TestMode::Confidence), 120_000, 400), None);
        // Ten random files say little, and say so.
        let g = guarantee(&t(TestMode::RandomFiles), 120_000, 10).unwrap();
        assert_eq!(g.damage.to_string(), "37%");
        assert_eq!(round_up("4.5321%".parse().unwrap()).to_string(), "4.6%");
        assert_eq!(round_up("0.0412%".parse().unwrap()).to_string(), "0.05%");
        assert_eq!(round_up("100%".parse().unwrap()).to_string(), "100%");
        // A backup small enough to be read whole.
        let g = guarantee(&t(TestMode::RandomFiles), 8, 8).unwrap();
        assert_eq!(g.damaged, 1);
        assert!(guarantee_words(&g).starts_with("if even one of its 8 files"));
        // Not a uniform sample: no claim.
        assert_eq!(guarantee(&t(TestMode::Percent), 1_000, 500), None);
        assert_eq!(guarantee(&t(TestMode::Rolling), 1_000, 500), None);
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
