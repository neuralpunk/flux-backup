//! How many files a restore test has to read for its result to mean something, and the
//! random sample itself.
//!
//! A snapshot has `N` files, of which an unknown `D` are damaged (can't be read back, or
//! don't match their record). A test reads `n` of them, picked uniformly at random without
//! replacement, and passes only if all `n` are sound. It misses the damage exactly when
//! every picked file is one of the `N − D` sound ones:
//!
//! ```text
//! P(miss) = C(N − D, n) / C(N, n) = ∏ (N − D − i) / (N − i),  i = 0 … n − 1
//! ```
//!
//! (the hypergeometric probability of drawing no damaged file). It falls as `D` grows, so
//! "a share `d` or more of the files is damaged" is hardest to catch at the smallest such
//! `D = ⌈d·N⌉`, and a sample sized for that `D` holds for every larger one. The size
//! asked for is the smallest `n` with `P(miss) ≤ 1 − c`, for a certainty `c`.
//!
//! For a large `N` this is close to `(1 − d)ⁿ ≤ 1 − c`, that is `n ≥ ln(1 − c) / ln(1 − d)`:
//! 459 files for 1% at 99%, whether the backup has ten thousand files or ten million. For
//! a small `N` the exact count is lower, because a file is never drawn twice.
//!
//! What the statement is, and isn't: before the sample is drawn, a backup with that much
//! damage would fail the test with probability at least `c`. It says nothing about damage
//! to fewer files than `D`; only reading every file does.

use flux_core::Percent;

/// The smallest number of damaged files that is "a share `damage` or more" of `total`:
/// `⌈d·N⌉`, and at least one.
pub fn damaged_at_least(total: u64, damage: Percent) -> u64 {
    let d = u128::from(damage.millionths());
    let n = (u128::from(total) * d).div_ceil(1_000_000) as u64;
    n.clamp(1, total.max(1))
}

/// ln of the chance that `n` of `total` files, picked at random without replacement, miss
/// all `damaged` of them: `ln( C(N − D, n) / C(N, n) )`. Negative infinity when a miss is
/// impossible (`n > N − D`).
pub fn ln_miss(total: u64, damaged: u64, n: u64) -> f64 {
    let (big_n, d, n) = (total, damaged.min(total), n.min(total));
    if n > big_n - d {
        return f64::NEG_INFINITY;
    }
    // C(N − D, n) / C(N, n) = C(N − n, D) / C(N, D): the same number, as a product of n
    // terms or of D terms. Take the shorter.
    let (a, steps) = if n <= d { (d, n) } else { (n, d) };
    (0..steps)
        .map(|i| (-(a as f64) / (big_n - i) as f64).ln_1p())
        .sum()
}

/// The chance of missing all `damaged` files with a sample of `n` (see [`ln_miss`]).
pub fn miss_probability(total: u64, damaged: u64, n: u64) -> f64 {
    ln_miss(total, damaged, n).exp()
}

/// ln(1 − c), with a hair of tolerance (a relative 10⁻¹²) so that a sample that meets the
/// certainty exactly isn't turned down by rounding in the sum of logarithms.
fn ln_allowed_miss(certainty: Percent) -> f64 {
    let alpha = f64::from(1_000_000 - certainty.millionths()) / 1e6;
    alpha.ln() * (1.0 - 1e-12)
}

/// How many of `total` files to test so that damage to a share `damage` or more of them
/// is caught with probability at least `certainty`: the smallest `n` with
/// `C(N − D, n) / C(N, n) ≤ 1 − c`, `D = ⌈d·N⌉`. Never more than `total`.
pub fn sample_size(total: u64, damage: Percent, certainty: Percent) -> u64 {
    if total == 0 {
        return 0;
    }
    let d = damaged_at_least(total, damage);
    let allowed = ln_allowed_miss(certainty);
    // The miss chance only falls as n grows, and is zero at n = N − D + 1: find where it
    // crosses, by halving.
    let (mut lo, mut hi) = (0u64, total - d + 1);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if ln_miss(total, d, mid) <= allowed {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// The other way round: a sample of `n` of `total` files passed. The fewest damaged files
/// it would have caught with probability at least `certainty` (`None` for an empty
/// sample). One, when every file was tested.
pub fn detectable(total: u64, n: u64, certainty: Percent) -> Option<u64> {
    if n == 0 || total == 0 {
        return None;
    }
    let n = n.min(total);
    let allowed = ln_allowed_miss(certainty);
    // The miss chance falls as D grows, and is zero at D = N − n + 1.
    let (mut lo, mut hi) = (1u64, total - n + 1);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if ln_miss(total, mid, n) <= allowed {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

/// `part` of `total` as a percentage, rounded **up** to a millionth: "this share or more"
/// then never claims more than "this many files or more" does.
pub fn share(part: u64, total: u64) -> Percent {
    if total == 0 {
        return Percent::from_millionths(0);
    }
    let m = (u128::from(part) * 1_000_000).div_ceil(u128::from(total));
    Percent::from_millionths(m.min(1_000_000) as u32)
}

/// A small random number generator for picking samples (xoshiro256++), seeded from the
/// system's randomness. Not for secrets: only so a sample is uniform and can't be
/// predicted from the clock.
pub struct Rng([u64; 4]);

impl Rng {
    pub fn from_os() -> Self {
        let mut seed = [0u8; 32];
        if getrandom::fill(&mut seed).is_err() {
            // No system randomness (hardly possible): the clock, spread out below.
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64);
            return Self::seeded(t);
        }
        let mut s = [0u64; 4];
        for (w, c) in s.iter_mut().zip(seed.as_chunks::<8>().0) {
            *w = u64::from_le_bytes(*c);
        }
        if s == [0; 4] {
            return Self::seeded(0);
        }
        Rng(s)
    }

    /// The same numbers for the same seed (tests). The seed is spread over the state with
    /// SplitMix64, as xoshiro's authors advise.
    pub fn seeded(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        Rng([next(), next(), next(), next()])
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.0;
        let out = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        out
    }

    /// A number below `bound`, each equally likely (Lemire's method: multiply, and draw
    /// again in the rare case that would favour some numbers).
    pub fn below(&mut self, bound: u64) -> u64 {
        debug_assert!(bound > 0);
        let mut m = u128::from(self.next_u64()) * u128::from(bound);
        if (m as u64) < bound {
            let threshold = bound.wrapping_neg() % bound;
            while (m as u64) < threshold {
                m = u128::from(self.next_u64()) * u128::from(bound);
            }
        }
        (m >> 64) as u64
    }
}

/// Move a uniform random sample of `n` items to the front of `v`, in random order (the
/// first `n` steps of a Fisher–Yates shuffle): every set of `n` is equally likely.
pub fn sample_to_front<T>(v: &mut [T], n: usize, rng: &mut Rng) {
    let len = v.len();
    for i in 0..n.min(len.saturating_sub(1)) {
        let j = i + rng.below((len - i) as u64) as usize;
        v.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(s: &str) -> Percent {
        s.parse().unwrap()
    }

    /// C(n, k), exactly.
    fn choose(n: u64, k: u64) -> u128 {
        if k > n {
            return 0;
        }
        (0..k).fold(1u128, |acc, i| acc * u128::from(n - i) / u128::from(i + 1))
    }

    #[test]
    fn miss_probability_is_the_hypergeometric_one() {
        // Every case of a small backup, against whole-number binomials.
        for total in 1..=40u64 {
            for damaged in 0..=total {
                for n in 0..=total {
                    let exact = choose(total - damaged, n) as f64 / choose(total, n) as f64;
                    let got = miss_probability(total, damaged, n);
                    assert!(
                        (got - exact).abs() <= 1e-12 * exact.max(1e-300),
                        "N={total} D={damaged} n={n}: {got} vs {exact}"
                    );
                }
            }
        }
        // Both ways of writing the product agree on a big one.
        let (a, b) = (
            ln_miss(10_000_000, 100_000, 459),
            ln_miss(10_000_000, 459, 100_000),
        );
        assert!((a - b).abs() < 1e-9, "{a} {b}");
    }

    #[test]
    fn the_sample_is_the_smallest_that_does() {
        // By brute force, with exact binomials: C(N−D, n)·10⁶ ≤ (10⁶ − c)·C(N, n).
        for total in [1u64, 2, 3, 7, 10, 25, 50, 64, 100] {
            for damage in ["0.01%", "1%", "5%", "10%", "33%", "50%"] {
                for certainty in ["50%", "90%", "95%", "99%", "99.9%", "99.99%"] {
                    let (d, c) = (pc(damage), pc(certainty));
                    let dmg = damaged_at_least(total, d);
                    let enough = |n: u64| {
                        choose(total - dmg, n) * 1_000_000
                            <= u128::from(1_000_000 - c.millionths()) * choose(total, n)
                    };
                    let want = (0..=total).find(|&n| enough(n)).unwrap();
                    assert_eq!(
                        sample_size(total, d, c),
                        want,
                        "N={total} d={damage} c={certainty}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_numbers_in_the_notes() {
        // ln(0.01) / ln(0.99) = 458.2: 459 files, almost whatever the size of the backup.
        let (d, c) = (pc("1%"), pc("99%"));
        assert_eq!(sample_size(10_000_000, d, c), 459);
        assert_eq!(sample_size(1_000_000, d, c), 459);
        assert_eq!(sample_size(100_000, d, c), 458);
        // Smaller backups need fewer: no file is drawn twice.
        assert_eq!(sample_size(10_000, d, c), 448);
        assert_eq!(sample_size(1_000, d, c), 368);
        // One damaged file in 100: 99 of them (the miss chance is (100 − n) / 100).
        assert_eq!(sample_size(100, d, c), 99);
        assert_eq!(sample_size(0, d, c), 0);
        assert_eq!(sample_size(1, d, c), 1);
        // Never more than there are, and growing with what's asked.
        let mut last = 0;
        for certainty in ["50%", "90%", "99%", "99.9%", "99.99%"] {
            let n = sample_size(5_000, pc("0.5%"), pc(certainty));
            assert!(n >= last && n <= 5_000);
            last = n;
        }
        // The large-N formula is an upper bound on the exact count.
        for total in [500u64, 5_000, 50_000, 5_000_000] {
            for (damage, certainty) in [("1%", "99%"), ("0.1%", "95%"), ("5%", "99.9%")] {
                let (d, c) = (pc(damage), pc(certainty));
                let bound = ((1.0 - c.fraction()).ln() / (1.0 - d.fraction()).ln()).ceil();
                assert!(sample_size(total, d, c) as f64 <= bound);
            }
        }
    }

    #[test]
    fn what_a_sample_would_have_caught() {
        let c = pc("99%");
        assert_eq!(detectable(1_000, 0, c), None);
        // Everything tested: any damage at all.
        assert_eq!(detectable(1_000, 1_000, c), Some(1));
        // Ten files of many: only damage to about 37% of them (1 − 0.01^(1/10)).
        let d = detectable(1_000_000, 10, c).unwrap();
        assert!((369_000..=369_100).contains(&d), "{d}");
        // It undoes sample_size: the sample for D catches D, and one file fewer doesn't.
        for total in [50u64, 1_000, 123_456] {
            for damage in ["0.5%", "1%", "10%"] {
                let dmg = damaged_at_least(total, pc(damage));
                let n = sample_size(total, pc(damage), c);
                assert!(detectable(total, n, c).unwrap() <= dmg);
                if n > 1 {
                    assert!(detectable(total, n - 1, c).unwrap() > dmg);
                }
            }
        }
        assert_eq!(share(1, 3).millionths(), 333_334, "rounded up");
        assert_eq!(share(5, 5).to_string(), "100%");
    }

    #[test]
    fn a_damaged_backup_is_caught_as_often_as_promised() {
        // 2,000 files, 1% of them damaged, tested 4,000 times with the sample for
        // (1%, 90%): the misses should be near the exact chance, and under 10%.
        let (total, d, c) = (2_000u64, pc("1%"), pc("90%"));
        let n = sample_size(total, d, c) as usize;
        let damaged = damaged_at_least(total, d);
        let expect = miss_probability(total, damaged, n as u64);
        assert!(expect <= 0.1);
        let mut rng = Rng::seeded(7);
        let mut files: Vec<u64> = (0..total).collect();
        let runs = 4_000;
        let mut missed = 0;
        for _ in 0..runs {
            sample_to_front(&mut files, n, &mut rng);
            // Files 0..damaged are the damaged ones.
            if files[..n].iter().all(|&f| f >= damaged) {
                missed += 1;
            }
        }
        let rate = f64::from(missed) / f64::from(runs);
        // Four standard deviations of a binomial around the exact chance.
        let sd = (expect * (1.0 - expect) / f64::from(runs)).sqrt();
        assert!((rate - expect).abs() < 4.0 * sd, "{rate} vs {expect}");
    }

    #[test]
    fn samples_are_uniform() {
        let mut rng = Rng::seeded(42);
        // below(): every value of a small range about equally often.
        let mut seen = [0u32; 7];
        for _ in 0..70_000 {
            seen[rng.below(7) as usize] += 1;
        }
        assert!(
            seen.iter().all(|&k| (9_500..=10_500).contains(&k)),
            "{seen:?}"
        );
        // Each of 20 items lands in a sample of 5 a quarter of the time.
        let mut hits = [0u32; 20];
        for _ in 0..40_000 {
            let mut v: Vec<usize> = (0..20).collect();
            sample_to_front(&mut v, 5, &mut rng);
            for &i in &v[..5] {
                hits[i] += 1;
            }
        }
        assert!(
            hits.iter().all(|&k| (9_500..=10_500).contains(&k)),
            "{hits:?}"
        );
        // Asking for more than there is, or of nothing, is harmless.
        let mut v = vec![1, 2, 3];
        sample_to_front(&mut v, 10, &mut rng);
        v.sort_unstable();
        assert_eq!(v, [1, 2, 3]);
        sample_to_front::<u8>(&mut [], 3, &mut rng);
    }
}
