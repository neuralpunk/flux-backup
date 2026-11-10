//! Unusual-change detection: after a backup, compare what changed with what usually
//! changes, and look for the marks ransomware leaves. A run that stands out is flagged in
//! words, the snapshot before it is pinned, and pruning holds off until it's dismissed.
//!
//! Signals:
//! * many more files changed or deleted than is usual for this backup, as Flux Sentry
//!   ([`crate::sentry`]) has learned it from the backup's history (fixed limits until
//!   there are five backups to learn from);
//! * files renamed to a new extension (`report.odt` → `report.odt.locked`), or many new
//!   files with extensions ransomware is known to use;
//! * text-like files (`.txt`, `.csv`…) whose new contents look encrypted (a sample is read:
//!   near-random bytes where text was);
//! * ransom notes (`HOW_TO_DECRYPT.txt`, `README_RESTORE_FILES`…).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use flux_core::profile::Sensitivity;
use flux_fs::entry::{Entry, Kind};

use crate::runlog::{RunRecord, RunStatus};

/// What was found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnusualReport {
    /// Each signal, in words.
    pub signals: Vec<String>,
    /// Files changed, deleted or added that look involved (at most 1000).
    pub affected: Vec<String>,
    pub affected_total: u64,
    /// Where they are: folder → how many (the 20 biggest).
    pub by_folder: Vec<(String, u64)>,
    /// Their extensions → how many (the 10 biggest).
    pub by_extension: Vec<(String, u64)>,
    /// The snapshot this backup made, and the one before it (pinned).
    pub snapshot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// The changes were seen in the files before the backup was written, and it wasn't
    /// made: `snapshot` is then not a snapshot's id but a name for this alarm
    /// (`before:<time>`), and `before` is the newest snapshot there is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// The snapshot from before (`before`) is pinned: it can't be removed by retention.
    /// False when there was none to pin, or it couldn't be pinned (said in the log).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub before_pinned: bool,
    /// Flux pinned it for this alarm (it wasn't pinned already): "This was me" takes
    /// that pin off again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned_by_flux: bool,
    /// Flux Sentry's reading of how much changed, against what's usual for this backup
    /// (when it had enough history to judge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentry: Option<crate::sentry::Reading>,
}

/// Extensions ransomware is known to add.
const RANSOM_EXTENSIONS: &[&str] = &[
    "locked",
    "encrypted",
    "enc",
    "crypt",
    "crypted",
    "crypto",
    "cry",
    "locky",
    "zepto",
    "cerber",
    "wncry",
    "wnry",
    "wcry",
    "lock",
    "crypz",
    "cryptolocker",
    "ryk",
    "conti",
];

/// Names of ransom notes (lowercase, without extension).
const NOTE_NAMES: &[&str] = &[
    "how_to_decrypt",
    "how-to-decrypt",
    "how_to_restore_files",
    "readme_decrypt",
    "decrypt_instructions",
    "decrypt-files",
    "_readme",
    "restore_files",
    "recover-files",
    "readme_to_restore",
    "!!!read_me!!!",
    "your_files_are_encrypted",
    "help_decrypt",
];

/// Extensions of files that are normally text (low entropy), so random-looking contents
/// stand out.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "tsv", "log", "json", "xml", "html", "htm", "svg", "rtf", "tex", "ini",
    "cfg", "conf", "yaml", "yml", "toml", "py", "rs", "js", "ts", "c", "h", "java", "sh", "sql",
    "css",
];

struct Thresholds {
    /// Share of files changed or deleted, at least, and times the usual share.
    share: f64,
    times_usual: f64,
    min_files: u64,
    renamed: u64,
    ransom_ext: u64,
    notes: u64,
    encrypted: u64,
}

fn thresholds(s: Sensitivity) -> Option<Thresholds> {
    match s {
        Sensitivity::Off => None,
        Sensitivity::Normal => Some(Thresholds {
            share: 0.30,
            times_usual: 5.0,
            min_files: 100,
            renamed: 20,
            ransom_ext: 10,
            notes: 3,
            encrypted: 10,
        }),
        Sensitivity::High => Some(Thresholds {
            share: 0.15,
            times_usual: 3.0,
            min_files: 30,
            renamed: 8,
            ransom_ext: 3,
            notes: 1,
            encrypted: 4,
        }),
    }
}

fn ext_of(path: &[u8]) -> Option<String> {
    let name = path.rsplit(|b| *b == b'/').next()?;
    let dot = name.iter().rposition(|b| *b == b'.')?;
    (dot > 0).then(|| String::from_utf8_lossy(&name[dot + 1..]).to_lowercase())
}

fn stem_lower(path: &[u8]) -> String {
    let name = path.rsplit(|b| *b == b'/').next().unwrap_or(path);
    let s = String::from_utf8_lossy(name).to_lowercase();
    match s.rfind('.') {
        Some(i) if i > 0 => s[..i].to_owned(),
        _ => s,
    }
}

fn folder_of(path: &[u8]) -> String {
    let s = String::from_utf8_lossy(path);
    match s.rfind('/') {
        Some(i) => format!("/{}", &s[..i]),
        None => "/".into(),
    }
}

/// Does the start of a file look like random bytes (encrypted)? By its entropy, against
/// what random data of that length has: a short sample can't reach 8 bits a byte (256
/// random bytes average about 7.2), so a fixed 7.5 never saw a small encrypted file.
pub fn looks_random(data: &[u8]) -> bool {
    if data.len() < 256 {
        return false;
    }
    // The expected entropy of n uniformly random bytes is about 8 - 255 / (2 n ln 2);
    // text sits near 4-5 whatever its length.
    let expected = 8.0 - 255.0 / (2.0 * data.len() as f64 * std::f64::consts::LN_2);
    entropy(data) >= (expected - 0.25).min(7.5)
}

/// Bits per byte (0 to 8): text is around 4–5, compressed or encrypted data near 8.
pub fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for b in data {
        counts[*b as usize] += 1;
    }
    let n = data.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = *c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// The usual share of files changed or deleted per backup, from the history (the median
/// of the last 10 backups that made a snapshot; `None` with fewer than 3).
fn usual_share(history: &[RunRecord]) -> Option<f64> {
    let mut shares: Vec<f64> = history
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                RunStatus::Completed | RunStatus::CompletedWithErrors
            )
        })
        .filter_map(|r| r.stats.as_ref())
        .filter(|s| s.files > 0)
        .map(|s| (s.changed + s.deleted) as f64 / s.files as f64)
        .take(10)
        .collect();
    if shares.len() < 3 {
        return None;
    }
    shares.sort_by(f64::total_cmp);
    Some(shares[shares.len() / 2])
}

/// Tripwire files that were changed or removed ([`crate::tripwire`]), as a signal, and
/// added to `report` (or a report of their own).
pub fn with_tripwires(
    report: Option<UnusualReport>,
    tripped: &[crate::tripwire::Tripped],
) -> Option<UnusualReport> {
    use crate::tripwire::Tripped;
    if tripped.is_empty() {
        return report;
    }
    let first = tripped[0].path().display();
    let signal = match tripped {
        [Tripped::Changed(_)] => format!("a tripwire file was changed ({first})"),
        [Tripped::Removed(_)] => format!("a tripwire file was removed ({first})"),
        many => format!(
            "{} tripwire files were changed or removed ({first}…)",
            many.len()
        ),
    };
    let mut r = report.unwrap_or(UnusualReport {
        signals: Vec::new(),
        affected: Vec::new(),
        affected_total: 0,
        by_folder: Vec::new(),
        by_extension: Vec::new(),
        snapshot: String::new(),
        before: None,
        stopped: false,
        before_pinned: false,
        pinned_by_flux: false,
        sentry: None,
    });
    r.signals.insert(0, signal);
    for t in tripped {
        let path = t.path().display().to_string();
        if !r.affected.contains(&path) {
            r.affected.insert(0, path);
            r.affected_total += 1;
        }
    }
    if r.by_folder.is_empty() {
        let mut folders: BTreeMap<String, u64> = BTreeMap::new();
        for t in tripped {
            let dir = t
                .path()
                .parent()
                .map_or(String::from("/"), |d| d.display().to_string());
            *folders.entry(dir).or_default() += 1;
        }
        r.by_folder = folders.into_iter().collect();
    }
    Some(r)
}

/// What Flux Sentry has learned of a backup from its earlier runs (`history`, newest
/// first: the runs that made a snapshot; see [`crate::sentry_history`] for which of them
/// to leave out). `None` until there are enough.
pub fn learn(history: &[RunRecord]) -> Option<crate::sentry::Baseline> {
    let runs: Vec<crate::sentry::Run> = history
        .iter()
        .rev()
        .filter(|r| {
            matches!(
                r.status,
                RunStatus::Completed | RunStatus::CompletedWithErrors
            )
        })
        .filter_map(|r| {
            // What the look counted, where there was one; the engine's statistics
            // otherwise (runs from before 1.1.1, or made without a look).
            r.sentry_seen.or_else(|| {
                r.stats.as_ref().map(|s| crate::sentry::Run {
                    touched: s.changed + s.deleted,
                    files: s.files,
                })
            })
        })
        .collect();
    crate::sentry::Baseline::learn(&runs)
}

/// What differs between two snapshots' files: what the signals are read from. Holds only
/// the files that changed, came or went (without their extended attributes), however
/// many files there are in all.
#[derive(Debug, Default)]
pub struct Diff {
    /// Files in the older list.
    pub files: u64,
    pub changed: Vec<Entry>,
    pub added: Vec<Entry>,
    pub deleted: Vec<Entry>,
}

/// Compare two lists of entries, **each sorted by path**, as they stream by: nothing is
/// held but the differences. `None` if a list turns out not to be sorted (the answer
/// would be nonsense).
pub fn diff(
    before: impl Iterator<Item = Entry>,
    after: impl Iterator<Item = Entry>,
) -> Option<Diff> {
    use std::cmp::Ordering;
    let slim = |mut e: Entry| {
        e.xattrs = Vec::new();
        e
    };
    // Files only, and in order.
    fn files(
        it: impl Iterator<Item = Entry>,
        sorted: &std::cell::Cell<bool>,
    ) -> impl Iterator<Item = Entry> {
        let mut last: Option<Vec<u8>> = None;
        it.filter(|e| e.kind == Kind::File).inspect(move |e| {
            if last.as_ref().is_some_and(|l| *l >= e.path) {
                sorted.set(false);
            }
            last = Some(e.path.clone());
        })
    }
    let sorted = std::cell::Cell::new(true);
    let mut old = files(before, &sorted).peekable();
    let mut new = files(after, &sorted).peekable();
    let mut d = Diff::default();
    loop {
        let order = match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(o), Some(n)) => o.path.cmp(&n.path),
        };
        match order {
            Ordering::Less => {
                d.files += 1;
                d.deleted.push(slim(old.next().expect("peeked")));
            }
            Ordering::Greater => d.added.push(slim(new.next().expect("peeked"))),
            Ordering::Equal => {
                d.files += 1;
                let (o, e) = (old.next().expect("peeked"), new.next().expect("peeked"));
                if o.size != e.size || o.mtime != e.mtime || (o.hash.is_some() && o.hash != e.hash)
                {
                    d.changed.push(slim(e));
                }
            }
        }
        if !sorted.get() {
            return None;
        }
    }
    Some(d)
}

/// Look at the change from `before` to `after` (a snapshot's entries, in any order).
/// `history`: this backup's earlier runs, newest first. `sample`: reads the start of a
/// file as it is now (for the "looks encrypted" signal).
pub fn analyze(
    before: &[Entry],
    after: &[Entry],
    history: &[RunRecord],
    sensitivity: Sensitivity,
    sample: &dyn Fn(&Path) -> Option<Vec<u8>>,
) -> Option<UnusualReport> {
    analyze_counted(before, after, history, sensitivity, sample).1
}

/// [`analyze`], and what it counted (files changed or deleted, of how many): `None`
/// when there was nothing to compare with.
pub fn analyze_counted(
    before: &[Entry],
    after: &[Entry],
    history: &[RunRecord],
    sensitivity: Sensitivity,
    sample: &dyn Fn(&Path) -> Option<Vec<u8>>,
) -> (Option<crate::sentry::Run>, Option<UnusualReport>) {
    let sorted = |v: &[Entry]| {
        let mut v: Vec<Entry> = v.iter().filter(|e| e.kind == Kind::File).cloned().collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v.dedup_by(|a, b| a.path == b.path);
        v
    };
    match diff(sorted(before).into_iter(), sorted(after).into_iter()) {
        Some(d) => analyze_diff(&d, history, sensitivity, sample),
        None => (None, None),
    }
}

/// Read the signals from what differs (see [`diff`]). Returns what was counted (files
/// changed or deleted, of how many; `None` when there was nothing to compare with), and
/// the report when the changes are unusual.
pub fn analyze_diff(
    d: &Diff,
    history: &[RunRecord],
    sensitivity: Sensitivity,
    sample: &dyn Fn(&Path) -> Option<Vec<u8>>,
) -> (Option<crate::sentry::Run>, Option<UnusualReport>) {
    let mut seen = None;
    let report = analyze_inner(d, history, sensitivity, sample, &mut seen);
    (seen, report)
}

fn analyze_inner(
    d: &Diff,
    history: &[RunRecord],
    sensitivity: Sensitivity,
    sample: &dyn Fn(&Path) -> Option<Vec<u8>>,
    seen: &mut Option<crate::sentry::Run>,
) -> Option<UnusualReport> {
    let t = thresholds(sensitivity)?;
    if d.files == 0 {
        return None; // a first backup has nothing to compare with
    }
    let files_before = d.files;
    let changed: Vec<&Entry> = d.changed.iter().collect();
    let added: Vec<&Entry> = d.added.iter().collect();
    let deleted: Vec<&Entry> = d.deleted.iter().collect();
    let mut signals = Vec::new();
    let mut affected: HashSet<Vec<u8>> = HashSet::new();

    // 1. Much more change than usual: by Flux Sentry, once it knows this backup; by
    // fixed limits until then.
    let touched = (changed.len() + deleted.len()) as u64;
    let share = touched as f64 / files_before as f64;
    *seen = Some(crate::sentry::Run {
        touched,
        files: files_before,
    });
    let mut sentry = None;
    let much = match (learn(history), crate::sentry::limits(sensitivity)) {
        (Some(baseline), Some(limits)) => {
            let reading = baseline.read(
                crate::sentry::Run {
                    touched,
                    files: files_before,
                },
                &limits,
            );
            sentry = Some(reading);
            reading.unusual.then(|| {
                format!(
                    "{touched} files changed or deleted, {} of them: {} what this backup usually changes ({})",
                    crate::sentry::share_words(share),
                    crate::sentry::times_words(reading.times),
                    crate::sentry::share_words(reading.usual),
                )
            })
        }
        _ => {
            let usual = usual_share(history);
            (touched >= t.min_files
                && share >= t.share
                && usual.is_none_or(|u| share >= u * t.times_usual))
            .then(|| {
                format!(
                    "{touched} files changed or deleted, {:.0}% of them{}",
                    share * 100.0,
                    usual.map_or(String::new(), |u| format!(" (usually {:.1}%)", u * 100.0))
                )
            })
        }
    };
    if let Some(words) = much {
        signals.push(words);
        affected.extend(changed.iter().map(|e| e.path.clone()));
        affected.extend(deleted.iter().map(|e| e.path.clone()));
    }

    // 2. Renamed to a new extension, or ransomware's extensions.
    let gone: HashSet<&[u8]> = deleted.iter().map(|e| e.path.as_slice()).collect();
    let mut renamed: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for e in &added {
        if let Some(dot) = e.path.iter().rposition(|b| *b == b'.')
            && gone.contains(&e.path[..dot])
            && let Some(ext) = ext_of(&e.path)
        {
            renamed.entry(ext).or_default().push(e);
        }
    }
    if let Some((ext, list)) = renamed.iter().max_by_key(|(_, v)| v.len())
        && list.len() as u64 >= t.renamed
    {
        signals.push(format!("{} files renamed to end in .{ext}", list.len()));
        for e in list {
            affected.insert(e.path.clone());
            if let Some(dot) = e.path.iter().rposition(|b| *b == b'.') {
                affected.insert(e.path[..dot].to_vec());
            }
        }
    }
    let ransom_ext: Vec<&&Entry> = added
        .iter()
        .filter(|e| ext_of(&e.path).is_some_and(|x| RANSOM_EXTENSIONS.contains(&x.as_str())))
        .collect();
    if ransom_ext.len() as u64 >= t.ransom_ext && !signals.iter().any(|s| s.contains("renamed")) {
        signals.push(format!(
            "{} new files with extensions ransomware uses (.{})",
            ransom_ext.len(),
            ext_of(&ransom_ext[0].path).unwrap_or_default()
        ));
        affected.extend(ransom_ext.iter().map(|e| e.path.clone()));
    }

    // 3. Ransom notes.
    let notes: Vec<&&Entry> = added
        .iter()
        .filter(|e| {
            let s = stem_lower(&e.path);
            NOTE_NAMES.iter().any(|n| s == *n || s.starts_with(n))
        })
        .collect();
    if notes.len() as u64 >= t.notes {
        signals.push(format!(
            "{} files that look like ransom notes ({})",
            notes.len(),
            String::from_utf8_lossy(notes[0].path.rsplit(|b| *b == b'/').next().unwrap_or(&[]))
        ));
        affected.extend(notes.iter().map(|e| e.path.clone()));
    }

    // 4. Text that now looks encrypted (a sample of what changed).
    let texts: Vec<&&Entry> = changed
        .iter()
        .filter(|e| ext_of(&e.path).is_some_and(|x| TEXT_EXTENSIONS.contains(&x.as_str())))
        .take(200)
        .collect();
    let random: Vec<&&&Entry> = texts
        .iter()
        .filter(|e| {
            sample(&PathBuf::from("/").join(e.rel_path())).is_some_and(|d| looks_random(&d))
        })
        .collect();
    if random.len() as u64 >= t.encrypted && random.len() * 2 >= texts.len() {
        signals.push(format!(
            "{} changed text files now look encrypted",
            random.len()
        ));
        affected.extend(random.iter().map(|e| e.path.clone()));
    }

    if signals.is_empty() {
        return None;
    }
    let mut affected: Vec<Vec<u8>> = affected.into_iter().collect();
    affected.sort();
    let mut by_folder: HashMap<String, u64> = HashMap::new();
    let mut by_ext: HashMap<String, u64> = HashMap::new();
    for p in &affected {
        *by_folder.entry(folder_of(p)).or_default() += 1;
        *by_ext
            .entry(ext_of(p).unwrap_or_else(|| "(none)".into()))
            .or_default() += 1;
    }
    let top = |m: HashMap<String, u64>, n: usize| {
        let mut v: Vec<(String, u64)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    };
    Some(UnusualReport {
        signals,
        affected_total: affected.len() as u64,
        affected: affected
            .iter()
            .take(1000)
            .map(|p| format!("/{}", String::from_utf8_lossy(p)))
            .collect(),
        by_folder: top(by_folder, 20),
        by_extension: top(by_ext, 10),
        snapshot: String::new(),
        before: None,
        stopped: false,
        before_pinned: false,
        pinned_by_flux: false,
        sentry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_fs::snapshot::SnapshotStats;

    fn file(path: &str, size: u64, mtime: i64) -> Entry {
        Entry {
            path: path.as_bytes().to_vec(),
            kind: Kind::File,
            mode: 0o644,
            uid: 0,
            gid: 0,
            size,
            mtime,
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

    fn tree(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| file(&format!("home/me/docs/f{i:04}.odt"), 1000 + i as u64, 1))
            .collect()
    }

    fn history(share: f64) -> Vec<RunRecord> {
        (0..5)
            .map(|_| RunRecord {
                run_id: "r".into(),
                profile: "p".into(),
                trigger: crate::Trigger::Scheduled,
                started: jiff::Timestamp::now(),
                finished: jiff::Timestamp::now(),
                status: RunStatus::Completed,
                snapshot: None,
                pruned: vec![],
                error: None,
                stats: Some(SnapshotStats {
                    files: 1000,
                    changed: (share * 1000.0) as u64,
                    ..Default::default()
                }),
                warnings: 0,
                hooks: vec![],
                free_after: None,
                sources_missing: vec![],
                destinations: vec![],
                unusual: None,
                sentry_seen: None,
                performance: None,
            })
            .collect()
    }

    const NONE: &dyn Fn(&Path) -> Option<Vec<u8>> = &|_| None;

    #[test]
    fn ordinary_change_isnt_flagged() {
        let before = tree(1000);
        let mut after = before.clone();
        for e in after.iter_mut().take(20) {
            e.mtime = 2;
        }
        assert!(analyze(&before, &after, &history(0.02), Sensitivity::Normal, NONE).is_none());
        // Off: never.
        let all_changed: Vec<Entry> = before
            .iter()
            .map(|e| file(&String::from_utf8_lossy(&e.path), 5, 9))
            .collect();
        assert!(
            analyze(
                &before,
                &all_changed,
                &history(0.02),
                Sensitivity::Off,
                NONE
            )
            .is_none()
        );
    }

    #[test]
    fn mass_change_against_the_usual() {
        let before = tree(1000);
        let mut after = before.clone();
        for e in after.iter_mut().take(400) {
            e.mtime = 2;
        }
        let r = analyze(&before, &after, &history(0.02), Sensitivity::Normal, NONE).unwrap();
        assert_eq!(
            r.signals,
            [
                "400 files changed or deleted, 40% of them: 20 times what this backup usually changes (2.0%)"
            ]
        );
        let reading = r.sentry.unwrap();
        assert!(reading.unusual && reading.runs == 5 && reading.seen == 0);
        // Before Sentry has five backups to learn from, the fixed limits judge.
        let r = analyze(
            &before,
            &after,
            &history(0.02)[..3],
            Sensitivity::Normal,
            NONE,
        )
        .unwrap();
        assert_eq!(
            r.signals,
            ["400 files changed or deleted, 40% of them (usually 2.0%)"]
        );
        assert_eq!(r.sentry, None);
        // What the fixed limits never saw: 8% in a backup that changes 0.5%.
        let mut some = before.clone();
        for e in some.iter_mut().take(80) {
            e.mtime = 2;
        }
        assert!(
            analyze(&before, &some, &history(0.005), Sensitivity::Normal, NONE).is_none(),
            "80 files: too few"
        );
        let big = tree(5000);
        let mut some = big.clone();
        for e in some.iter_mut().take(400) {
            e.mtime = 2;
        }
        let r = analyze(&big, &some, &history(0.005), Sensitivity::Normal, NONE).unwrap();
        assert_eq!(
            r.signals,
            [
                "400 files changed or deleted, 8.0% of them: 15 times what this backup usually changes (0.55%)"
            ]
        );
        assert_eq!(r.affected_total, 400);
        assert_eq!(r.by_folder[0], ("/home/me/docs".into(), 400));
        // A backup that always changes a lot: 40% is its normal.
        assert!(analyze(&before, &after, &history(0.35), Sensitivity::Normal, NONE).is_none());
    }

    #[test]
    fn renamed_to_a_new_extension_and_notes() {
        let before = tree(100);
        let mut after: Vec<Entry> = before[30..].to_vec();
        for e in &before[..30] {
            after.push(file(
                &format!("{}.locked", String::from_utf8_lossy(&e.path)),
                e.size + 16,
                2,
            ));
        }
        after.push(file("home/me/docs/HOW_TO_DECRYPT.txt", 900, 2));
        after.push(file("home/me/HOW_TO_DECRYPT.txt", 900, 2));
        after.push(file("home/me/pics/HOW_TO_DECRYPT.txt", 900, 2));
        let r = analyze(&before, &after, &[], Sensitivity::Normal, NONE).unwrap();
        assert!(
            r.signals
                .iter()
                .any(|s| s == "30 files renamed to end in .locked"),
            "{:?}",
            r.signals
        );
        assert!(
            r.signals
                .iter()
                .any(|s| s.starts_with("3 files that look like ransom notes"))
        );
        assert!(
            r.by_extension
                .iter()
                .any(|(x, n)| x == "locked" && *n == 30)
        );
        assert!(
            r.affected.iter().any(|p| p == "/home/me/docs/f0000.odt"),
            "the originals are listed"
        );
    }

    /// The two lists are compared as they stream by: what differs is all that's kept,
    /// and the answer is the same as holding both. A list out of order is refused.
    #[test]
    fn the_streaming_diff_finds_what_changed_came_and_went() {
        let before = vec![
            file("a/gone.txt", 10, 1),
            file("a/same.txt", 10, 1),
            file("a/touched.txt", 10, 1),
            file("b/grown.txt", 10, 1),
        ];
        let mut dir = file("a", 0, 1);
        dir.kind = Kind::Dir;
        let after = vec![
            dir,
            file("a/new.txt", 5, 3),
            file("a/same.txt", 10, 1),
            file("a/touched.txt", 10, 2),
            file("b/grown.txt", 12, 1),
        ];
        let d = diff(before.iter().cloned(), after.iter().cloned()).unwrap();
        let names = |v: &[Entry]| -> Vec<String> {
            v.iter()
                .map(|e| String::from_utf8_lossy(&e.path).into_owned())
                .collect()
        };
        assert_eq!(d.files, 4);
        assert_eq!(names(&d.changed), ["a/touched.txt", "b/grown.txt"]);
        assert_eq!(names(&d.added), ["a/new.txt"]);
        assert_eq!(names(&d.deleted), ["a/gone.txt"]);
        // Out of order: no answer, rather than a wrong one.
        let mut jumbled = before.clone();
        jumbled.swap(0, 3);
        assert!(diff(jumbled.into_iter(), after.into_iter()).is_none());
    }

    /// A small encrypted file is as random as a big one, but a short sample's entropy
    /// can't reach what a long one's does: the bar follows the length.
    #[test]
    fn small_files_can_look_encrypted_too() {
        // xorshift: bytes as good as random for this.
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    (x >> 24) as u8
                })
                .collect()
        };
        for n in [256, 300, 400, 1000, 65536] {
            let hits = (0..50).filter(|_| looks_random(&noise(n))).count();
            assert!(hits >= 48, "{n} bytes: {hits} of 50");
        }
        assert!(!looks_random(&noise(200)), "too short to tell");
        let text = "The quick brown fox jumps over the lazy dog, again and again. ".repeat(8);
        assert!(!looks_random(text.as_bytes()));
        // Base64 (6 bits a byte) is text too.
        let b64: Vec<u8> = noise(600)
            .iter()
            .map(|b| {
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                    [(b & 63) as usize]
            })
            .collect();
        assert!(!looks_random(&b64));
    }

    #[test]
    fn text_that_looks_encrypted() {
        let before: Vec<Entry> = (0..20)
            .map(|i| file(&format!("home/me/notes/n{i}.txt"), 5000, 1))
            .collect();
        let after: Vec<Entry> = before
            .iter()
            .map(|e| file(&String::from_utf8_lossy(&e.path), 5016, 2))
            .collect();
        let random: Vec<u8> = (0..65536u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        assert!(entropy(&random) > 7.9);
        assert!(entropy(b"plain words in a plain text file, nothing hidden here at all") < 5.0);
        let encrypted = |_: &Path| Some(random.clone());
        let r = analyze(
            &before,
            &after,
            &history(0.9),
            Sensitivity::Normal,
            &encrypted,
        )
        .unwrap();
        assert_eq!(r.signals, ["20 changed text files now look encrypted"]);
        let plain = |_: &Path| Some(b"dear diary, today I wrote some more plain text. ".repeat(20));
        assert!(analyze(&before, &after, &history(0.9), Sensitivity::Normal, &plain).is_none());
    }
}
