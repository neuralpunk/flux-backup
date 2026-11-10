//! Flux Sentry: learns how much of a backup normally changes from one run to the next,
//! and says when a run doesn't fit. (The "much more change than usual" signal of
//! [`crate::unusual`]; the marks of ransomware are looked for there.)
//!
//! Fixed limits ("30% of the files changed") fit no backup well: a folder of finished
//! work where nothing ever changes should raise the alarm long before that, and a
//! projects folder that is rebuilt every Monday shouldn't raise it at all. Sentry judges
//! each run against the backup's own history instead. It keeps three things.
//!
//! **A level.** Each run's change is the share of the files that changed or were deleted,
//! on a logarithmic scale, since shares of 0.1% and of 10% are both ordinary, for
//! different backups: `y = ln((changed + ½) / (files + 1))`. The level `μ` is an
//! exponentially weighted moving average of `y`, so it follows a backup whose habits
//! drift and forgets old ones:
//!
//! ```text
//! μ ← μ + α · clamp(y − μ, ±3σ)          α = 0.2
//! ```
//!
//! The step is clamped (a Huber step), so one extraordinary run moves the level only a
//! little, however large it was.
//!
//! **A spread.** How far runs usually land from the level: the median absolute deviation
//! of the last 30 forecast errors `y − μ` (each measured before the level took that run
//! in), times 1.4826, which makes it an estimate of the standard deviation that up to
//! half of the runs being outliers can't inflate. It is kept between 0.3 and
//! `ln 20 / k`, so that a metronome of a backup doesn't alarm at a breath of change, and
//! an erratic one still alarms at twenty times its usual.
//!
//! **A memory.** A level of change this backup has reached before, in at least two runs
//! that weren't flagged (or were, and the owner said "this was me"), is part of its
//! normal, however rare: the Monday rebuild, the yearly clear-out.
//!
//! A run is unusual when its score `z = (y − μ) / σ` reaches `k` (6, or 4 when watching
//! closely), it touches enough files to matter, and the memory doesn't know it. With the
//! spread at its smallest that is 6 times the usual change (3.3 times, closely); for a
//! backup whose runs vary more, proportionally more.

use flux_core::profile::Sensitivity;

/// The name it goes by.
pub const NAME: &str = "Flux Sentry";

/// How fast the level follows: each run counts for a fifth of its distance from it.
const ALPHA: f64 = 0.2;
/// Forecast errors the spread is taken from.
const WINDOW: usize = 30;
/// Runs needed before Sentry judges (until then, fixed limits do).
pub const WARM_UP: usize = 5;
/// The smallest spread: e^(6 × 0.3) ≈ 6 times the usual, at the least.
const SPREAD_MIN: f64 = 0.3;
/// At this many times the usual, every backup alarms, however erratic.
const TIMES_MAX: f64 = 20.0;
/// The Huber step: errors beyond this many spreads count as this many.
const CLAMP: f64 = 3.0;
/// MAD to standard deviation, for a normal distribution: 1 / Φ⁻¹(¾).
const MAD_TO_SIGMA: f64 = 1.4826;
/// "Reached before": a run with at least this much of today's change…
const SEEN_SHARE: f64 = 0.8;
/// …in this many earlier runs.
const SEEN_RUNS: usize = 2;

/// One run of the history: files changed or deleted, of how many.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub touched: u64,
    pub files: u64,
}

impl Run {
    /// The share of the files that changed.
    pub fn share(&self) -> f64 {
        if self.files == 0 {
            0.0
        } else {
            self.touched as f64 / self.files as f64
        }
    }

    /// The change on Sentry's scale: the logarithm of the share, with half a file added
    /// so that a run with no changes has one.
    fn y(&self) -> f64 {
        ((self.touched as f64 + 0.5) / (self.files as f64 + 1.0)).ln()
    }
}

/// What Sentry asks of a run before calling it unusual.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The score to reach.
    pub k: f64,
    /// Files changed or deleted, at least.
    pub min_files: u64,
    /// And this share of the files, at least.
    pub min_share: f64,
}

pub fn limits(s: Sensitivity) -> Option<Limits> {
    match s {
        Sensitivity::Off => None,
        Sensitivity::Normal => Some(Limits {
            k: 6.0,
            min_files: 100,
            min_share: 0.05,
        }),
        Sensitivity::High => Some(Limits {
            k: 4.0,
            min_files: 30,
            min_share: 0.02,
        }),
    }
}

/// What a backup's history taught.
#[derive(Debug, Clone, PartialEq)]
pub struct Baseline {
    /// The level `μ`: the logarithm of the usual share.
    level: f64,
    /// 1.4826 × the median absolute forecast error, before its bounds.
    spread: f64,
    /// The runs it was learned from, oldest first.
    runs: Vec<Run>,
}

/// A run, judged.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Reading {
    /// The share of files this backup usually changes.
    pub usual: f64,
    /// This run's share.
    pub share: f64,
    /// `share / usual`.
    pub times: f64,
    /// How many spreads above the level.
    pub score: f64,
    /// Runs the baseline was learned from.
    pub runs: usize,
    /// Earlier runs that changed about as much or more.
    pub seen: usize,
    pub unusual: bool,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => v[n / 2],
        _ => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

impl Baseline {
    /// Learn from a backup's earlier runs, oldest first. `None` until there are
    /// [`WARM_UP`] of them.
    pub fn learn(runs: &[Run]) -> Option<Self> {
        let runs: Vec<Run> = runs.iter().copied().filter(|r| r.files > 0).collect();
        if runs.len() < WARM_UP {
            return None;
        }
        // Start at the median of the first runs (one odd first backup doesn't set it),
        // then walk the history as it happened.
        let mut level = median(runs[..WARM_UP].iter().map(Run::y).collect());
        let mut errors: Vec<f64> = Vec::with_capacity(runs.len());
        for r in &runs {
            let error = r.y() - level;
            let recent = &errors[errors.len().saturating_sub(WINDOW)..];
            // The spread so far; generous while there is little to go on.
            let spread = if recent.len() < WARM_UP {
                1.0
            } else {
                (MAD_TO_SIGMA * median(recent.iter().map(|e| e.abs()).collect())).max(SPREAD_MIN)
            };
            level += ALPHA * error.clamp(-CLAMP * spread, CLAMP * spread);
            errors.push(error);
        }
        let recent = &errors[errors.len().saturating_sub(WINDOW)..];
        Some(Self {
            level,
            spread: MAD_TO_SIGMA * median(recent.iter().map(|e| e.abs()).collect()),
            runs,
        })
    }

    /// The share of its files this backup usually changes.
    pub fn usual(&self) -> f64 {
        self.level.exp()
    }

    /// Runs it was learned from.
    pub fn runs(&self) -> usize {
        self.runs.len()
    }

    /// The spread used against a score of `k`: within its bounds.
    fn spread(&self, k: f64) -> f64 {
        self.spread.clamp(SPREAD_MIN, TIMES_MAX.ln() / k)
    }

    /// The share at which a run becomes unusual (before the memory has its say): what
    /// the settings can show as "it would warn at about …". `None` when no share of the
    /// files would: the mark lies beyond all of them (a backup that already changes much
    /// and widely), or the backup has fewer files than a warning needs changed. Such a
    /// backup is warned about by the marks of ransomware only.
    pub fn alarm_share(&self, l: &Limits) -> Option<f64> {
        let files = self.runs.last().map_or(0, |r| r.files);
        let by_count = if files == 0 {
            f64::INFINITY
        } else {
            l.min_files as f64 / files as f64
        };
        let share = (self.level + l.k * self.spread(l.k))
            .exp()
            .max(l.min_share)
            .max(by_count);
        (share <= 1.0).then_some(share)
    }

    /// Judge a run.
    pub fn read(&self, run: Run, l: &Limits) -> Reading {
        let score = (run.y() - self.level) / self.spread(l.k);
        let share = run.share();
        let seen = self
            .runs
            .iter()
            .filter(|r| r.share() >= share * SEEN_SHARE)
            .count();
        Reading {
            usual: self.usual(),
            share,
            times: share / self.usual(),
            score,
            runs: self.runs.len(),
            seen,
            unusual: score >= l.k
                && run.touched >= l.min_files
                && share >= l.min_share
                && seen < SEEN_RUNS,
        }
    }
}

/// "40 times", "6.5 times".
pub fn times_words(times: f64) -> String {
    if times >= 10.0 {
        format!("{times:.0} times")
    } else {
        format!("{times:.1} times")
    }
}

/// "0.12%", "4.8%", "40%".
pub fn share_words(share: f64) -> String {
    let p = share * 100.0;
    if p >= 10.0 {
        format!("{p:.0}%")
    } else if p >= 1.0 {
        format!("{p:.1}%")
    } else if p >= 0.1 {
        format!("{p:.2}%")
    } else {
        "under 0.1%".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: Limits = Limits {
        k: 6.0,
        min_files: 100,
        min_share: 0.05,
    };
    const CLOSELY: Limits = Limits {
        k: 4.0,
        min_files: 30,
        min_share: 0.02,
    };

    /// Repeatable noise: uniform in [0, 1), and normal by Box–Muller.
    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
        }
        fn normal(&mut self) -> f64 {
            let (u, v) = (self.uniform().max(1e-300), self.uniform());
            (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
        }
    }

    fn run(share: f64, files: u64) -> Run {
        Run {
            touched: (share * files as f64).round() as u64,
            files,
        }
    }

    /// A history around `share`, each run off by a factor e^(sigma·N(0,1)).
    fn history(share: f64, sigma: f64, n: usize, files: u64, seed: u64) -> Vec<Run> {
        let mut noise = Noise(seed);
        (0..n)
            .map(|_| run((share * (sigma * noise.normal()).exp()).min(1.0), files))
            .collect()
    }

    #[test]
    fn it_learns_the_level_and_the_spread() {
        assert_eq!(
            Baseline::learn(&history(0.02, 0.2, WARM_UP - 1, 10_000, 1)),
            None
        );
        // A steady backup: the level is its share, the spread its scatter.
        for (share, sigma) in [(0.001, 0.4), (0.02, 0.5), (0.3, 0.35)] {
            let b = Baseline::learn(&history(share, sigma, 60, 200_000, 7)).unwrap();
            assert!(
                (b.usual() / share).ln().abs() < 0.3,
                "level {} for {share}",
                b.usual()
            );
            assert!(
                (b.spread / sigma - 1.0).abs() < 0.45,
                "spread {} for {sigma}",
                b.spread
            );
        }
        // Identical runs: no spread at all, and the floor stands in.
        let b = Baseline::learn(&[run(0.02, 1_000); 12]).unwrap();
        assert!(b.spread < 1e-9 && (b.usual() - 0.02).abs() < 0.001);
        assert_eq!(b.spread(6.0), SPREAD_MIN);
        // Runs with no files say nothing.
        assert_eq!(Baseline::learn(&[run(0.5, 0); 20]), None);
    }

    #[test]
    fn a_quiet_backup_alarms_early_and_a_busy_one_late() {
        // Finished work: 0.2% changes, steadily. 6% is thirty times that.
        let quiet = Baseline::learn(&history(0.002, 0.3, 40, 50_000, 3)).unwrap();
        let r = quiet.read(run(0.06, 50_000), &NORMAL);
        assert!(r.unusual && r.times > 20.0 && r.score > 6.0, "{r:?}");
        // The fixed limit this replaces waited for 30%.
        assert!(quiet.alarm_share(&NORMAL).unwrap() <= 0.05 + 1e-9);
        // But a handful of files isn't an alarm, whatever the factor.
        assert!(!quiet.read(run(0.012, 5_000), &NORMAL).unusual, "60 files");
        assert!(!quiet.read(run(0.03, 50_000), &NORMAL).unusual, "under 5%");
        assert!(
            quiet.read(run(0.03, 50_000), &CLOSELY).unusual,
            "closely: 2%"
        );

        // A projects folder: around 8%, but anything from 2% to 30% happens. 35% is
        // nothing new for it; the fixed limit (30%, and 5 times the median) is close to
        // alarming on every big day.
        let busy = Baseline::learn(&history(0.08, 0.8, 60, 20_000, 5)).unwrap();
        let r = busy.read(run(0.35, 20_000), &NORMAL);
        assert!(!r.unusual && r.score < 6.0, "{r:?}");
        // For a backup this erratic the alarm is at twenty times the usual, which is
        // more than everything: the marks of ransomware (renames, encrypted text, notes)
        // are what protect it, not the amount of change.
        // (So "it would warn at about…" has nothing to say: no share of the files would.)
        assert_eq!(busy.alarm_share(&NORMAL), None);
        // Nor has it for a backup with fewer files than a warning needs changed.
        let small = Baseline::learn(&[run(0.02, 50); 30]).unwrap();
        assert_eq!(small.alarm_share(&NORMAL), None);

        // A metronome: 2% every time. Five times that isn't six; seven is.
        let steady = Baseline::learn(&[run(0.02, 10_000); 30]).unwrap();
        assert!(!steady.read(run(0.10, 10_000), &NORMAL).unusual);
        assert!(steady.read(run(0.14, 10_000), &NORMAL).unusual);
        assert!(
            steady.read(run(0.10, 10_000), &CLOSELY).unusual,
            "3.3 times, closely"
        );
        assert!((steady.alarm_share(&NORMAL).unwrap() - 0.02 * (1.8f64).exp()).abs() < 0.002);
    }

    #[test]
    fn what_it_has_seen_twice_is_normal() {
        // Quiet all week, a rebuild every Monday that touches a third of the files.
        let week = |seed: u64| {
            let mut w = history(0.004, 0.3, 6, 30_000, seed);
            w.push(run(0.33, 30_000));
            w
        };
        // The first Monday in the history: one such run isn't a habit yet.
        let mut runs: Vec<Run> = history(0.004, 0.3, 20, 30_000, 1);
        let b = Baseline::learn(&runs).unwrap();
        assert!(b.read(run(0.33, 30_000), &NORMAL).unusual);
        runs.extend(week(2));
        let b = Baseline::learn(&runs).unwrap();
        let r = b.read(run(0.33, 30_000), &NORMAL);
        assert!(r.unusual && r.seen == 1, "{r:?}");
        // Two Mondays behind it (accepted: not flagged, or "this was me"): it knows.
        runs.extend(week(3));
        let b = Baseline::learn(&runs).unwrap();
        let r = b.read(run(0.33, 30_000), &NORMAL);
        assert!(!r.unusual && r.seen == 2 && r.score > 6.0, "{r:?}");
        // The Mondays didn't drag the level up: the median ignores them, and the step
        // toward each was clamped. An ordinary day's change is still the usual.
        assert!(b.usual() < 0.012, "{}", b.usual());
        // And what it hasn't seen is still unusual: twice the Monday rebuild.
        assert!(b.read(run(0.70, 30_000), &NORMAL).unusual);
    }

    #[test]
    fn it_follows_a_backup_that_changes_its_habits() {
        // 1% for a long time, then the backup takes on a busy folder: 12% from now on.
        let mut runs = history(0.01, 0.3, 40, 40_000, 1);
        let before = Baseline::learn(&runs).unwrap();
        assert!(before.read(run(0.12, 40_000), &NORMAL).unusual);
        runs.extend(history(0.12, 0.3, 25, 40_000, 2));
        let after = Baseline::learn(&runs).unwrap();
        assert!(
            (after.usual() / 0.12).ln().abs() < 0.5,
            "the level moved to the new normal: {}",
            after.usual()
        );
        assert!(!after.read(run(0.15, 40_000), &NORMAL).unusual);
        // One wild run alone moves the level by at most α·3σ (here e^(0.2·3·0.3) = 1.2).
        let mut spiked = history(0.01, 0.3, 40, 40_000, 1);
        spiked.push(run(0.95, 40_000));
        let b = Baseline::learn(&spiked).unwrap();
        assert!(
            b.usual() / before.usual() < 1.35,
            "{} {}",
            b.usual(),
            before.usual()
        );
    }

    #[test]
    fn false_alarms_are_rare() {
        // Ordinary histories of every temper, each followed by 400 ordinary runs: how
        // often does an ordinary run look unusual? Lognormal scatter, files enough that
        // the floors don't hide anything.
        let mut noise = Noise(11);
        let (mut alarms, mut judged) = (0u32, 0u32);
        for trial in 0..40 {
            let share = 0.0005 * (1.0 + 99.0 * noise.uniform()); // 0.05% to 5%
            let sigma = 0.15 + 0.75 * noise.uniform();
            let mut runs = history(share, sigma, 30, 1_000_000, 100 + trial);
            for r in history(share, sigma, 400, 1_000_000, 500 + trial) {
                let b = Baseline::learn(&runs).unwrap();
                let floorless = Limits {
                    min_files: 0,
                    min_share: 0.0,
                    ..NORMAL
                };
                judged += 1;
                if b.read(r, &floorless).unusual {
                    alarms += 1;
                } else {
                    runs.push(r);
                }
                if runs.len() > 60 {
                    runs.remove(0);
                }
            }
        }
        // Six spreads out, with a spread that half the runs can't inflate: well under
        // one ordinary run in a thousand, even without the floors.
        assert!(alarms * 1000 < judged, "{alarms} alarms in {judged} runs");
    }

    #[test]
    fn words() {
        assert_eq!(times_words(41.6), "42 times");
        assert_eq!(times_words(6.52), "6.5 times");
        assert_eq!(share_words(0.4), "40%");
        assert_eq!(share_words(0.048), "4.8%");
        assert_eq!(share_words(0.0012), "0.12%");
        assert_eq!(share_words(0.0002), "under 0.1%");
        assert!(limits(Sensitivity::Off).is_none());
        assert_eq!(limits(Sensitivity::Normal), Some(NORMAL));
        assert_eq!(limits(Sensitivity::High), Some(CLOSELY));
    }
}
