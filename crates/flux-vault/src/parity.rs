//! Repair data: Reed–Solomon parity for a stored file, kept in a file of its own
//! (`parity/<the file's path>`), so that damage to part of the file can be undone, not
//! only detected.
//!
//! The file is cut into blocks of 4 KiB. The blocks are dealt out to `G` *stripes* in
//! turn (block `i` goes to stripe `i mod G`, as its column `i div G`), so a run of
//! damaged blocks is spread over all the stripes rather than landing in one. Each stripe
//! of `k` data blocks gets `m` parity blocks, with `k + m ≤ 256`:
//!
//! ```text
//! parity[j] = Σ  C[j][c] · data[c]        C[j][c] = 1 / (x_j + y_c),  x_j = 255 − j,  y_c = c
//!            c<k
//! ```
//!
//! byte by byte in GF(2⁸) ([`crate::gf256`]). `C` is a Cauchy matrix: the `x`s and `y`s
//! are all different (`c + j ≤ k + m − 2 < 255`), and every square part of a Cauchy
//! matrix has an inverse. So whichever `e ≤ m` data blocks of a stripe are lost, any `e`
//! parity blocks give `e` equations in the `e` lost blocks that can always be solved: the
//! code is MDS, no code with `m` parity blocks repairs more.
//!
//! Which blocks are lost is known from a hash of every block, data and parity, kept in
//! the parity file's *head*; a block that doesn't match (or can't be read) is treated as
//! lost. The head also holds the BLAKE3 of the whole file, which every repair is checked
//! against before it is used. The head is stored twice, at both ends of the parity file,
//! each copy with its own checksum.
//!
//! What it repairs: up to `m` damaged blocks in every stripe. So any `m` blocks anywhere
//! in the file, any one run of up to `m·G` blocks (about the share of the file that the
//! parity takes), and usually far more scattered damage than `m` blocks. What it doesn't:
//! a file that is gone, or damaged beyond that; a copy elsewhere is for those.

use rayon::prelude::*;

use crate::gf256;

/// The size of a block: a sector of today's drives.
pub const BLOCK: usize = 4096;
const MAGIC: &[u8; 8] = b"FLUXPAR1";
/// Bytes of a block's BLAKE3 that are kept.
const HASH: usize = 16;
/// The head before its tables: magic, block size, stripes, m, percent, length, file hash.
const FIXED: usize = 8 + 4 + 4 + 2 + 2 + 8 + 32;
/// The most parity that can be asked for, in percent of the file.
pub const MAX_PERCENT: u8 = 50;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParityError {
    /// The parity file itself can't be used (both copies of its head are damaged, or it
    /// isn't a parity file).
    #[error("the repair data can't be read: {0}")]
    Unusable(&'static str),
    /// More blocks are lost than the parity can rebuild.
    #[error(
        "too much is damaged to repair: {lost} blocks of one group are lost and {spare} can be rebuilt ({bad_data} damaged blocks in all)"
    )]
    TooMuch {
        lost: usize,
        spare: usize,
        bad_data: usize,
    },
    /// The rebuilt file isn't the one the parity was made for.
    #[error("the repaired data doesn't match its checksum")]
    Mismatch,
}

/// How a file of `len` bytes is laid out for parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub len: u64,
    pub block: usize,
    /// Stripes (`G`); 0 for an empty file.
    pub stripes: usize,
    /// Parity blocks per stripe.
    pub m: usize,
}

impl Geometry {
    /// The layout for a new parity file: `percent` of the file as parity (1 to
    /// [`MAX_PERCENT`]), in stripes as large as GF(2⁸) allows, and at least two parity
    /// blocks to a stripe.
    pub fn plan(len: u64, percent: u8) -> Self {
        let p = usize::from(percent.clamp(1, MAX_PERCENT));
        let n = len.div_ceil(BLOCK as u64) as usize;
        if n == 0 {
            return Self {
                len,
                block: BLOCK,
                stripes: 0,
                m: 0,
            };
        }
        // The most data blocks in a stripe with room for their parity: K(1 + p/100) ≤ 255.
        let k_max = 25_500 / (100 + p);
        let stripes = n.div_ceil(k_max);
        let k = n.div_ceil(stripes);
        Self {
            len,
            block: BLOCK,
            stripes,
            m: (k * p).div_ceil(100).max(2),
        }
    }

    /// Data blocks in the file.
    pub fn blocks(&self) -> usize {
        self.len.div_ceil(self.block as u64) as usize
    }

    /// Data blocks in the largest stripe.
    fn k(&self) -> usize {
        if self.stripes == 0 {
            0
        } else {
            self.blocks().div_ceil(self.stripes)
        }
    }

    pub fn parity_blocks(&self) -> usize {
        self.stripes * self.m
    }

    fn head_len(&self) -> usize {
        FIXED + HASH * (self.blocks() + self.parity_blocks()) + 32
    }

    /// The size of the parity file.
    pub fn file_len(&self) -> usize {
        2 * self.head_len() + self.parity_blocks() * self.block + 8
    }

    /// The bytes of data block `i`.
    fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = i * self.block;
        start..(start + self.block).min(self.len as usize)
    }

    /// Where parity block `j` of stripe `g` is, among the parity blocks: row by row, so
    /// that a run of damage in the parity file is spread over the stripes too.
    fn parity_index(&self, j: usize, g: usize) -> usize {
        j * self.stripes + g
    }
}

/// `C[j][c]`: the weight of data column `c` in parity row `j`.
fn coefficient(j: usize, c: usize) -> u8 {
    debug_assert!(c + j < 255);
    gf256::inv((255 - j as u8) ^ c as u8)
}

fn block_hash(block: &[u8]) -> [u8; HASH] {
    let mut h = [0u8; HASH];
    h.copy_from_slice(&blake3::hash(block).as_bytes()[..HASH]);
    h
}

/// The parity blocks for `data`, one after another (see [`Geometry::parity_index`]).
fn parity_of(data: &[u8], geo: &Geometry) -> Vec<u8> {
    let (b, g_count) = (geo.block, geo.stripes);
    let mut parity = vec![0u8; geo.parity_blocks() * b];
    if parity.is_empty() {
        return parity;
    }
    // One row of parity blocks (the j-th of every stripe) per task.
    parity
        .par_chunks_mut(g_count * b)
        .enumerate()
        .for_each(|(j, row)| {
            for i in 0..geo.blocks() {
                let (g, c) = (i % g_count, i / g_count);
                let src = &data[geo.range(i)];
                gf256::mul_add(&mut row[g * b..g * b + src.len()], coefficient(j, c), src);
            }
        });
    parity
}

/// The parity file for `data`: `percent` of its size, and a little for the hashes.
pub fn encode(data: &[u8], percent: u8) -> Vec<u8> {
    let geo = Geometry::plan(data.len() as u64, percent);
    let parity = parity_of(data, &geo);
    let mut head = Vec::with_capacity(geo.head_len());
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&(geo.block as u32).to_le_bytes());
    head.extend_from_slice(&(geo.stripes as u32).to_le_bytes());
    head.extend_from_slice(&(geo.m as u16).to_le_bytes());
    head.extend_from_slice(&u16::from(percent.clamp(1, MAX_PERCENT)).to_le_bytes());
    head.extend_from_slice(&geo.len.to_le_bytes());
    head.extend_from_slice(blake3::hash(data).as_bytes());
    let data_hashes: Vec<[u8; HASH]> = (0..geo.blocks())
        .into_par_iter()
        .map(|i| block_hash(&data[geo.range(i)]))
        .collect();
    data_hashes.iter().for_each(|h| head.extend_from_slice(h));
    for p in parity.chunks(geo.block) {
        head.extend_from_slice(&block_hash(p));
    }
    let sum = blake3::hash(&head);
    head.extend_from_slice(sum.as_bytes());
    debug_assert_eq!(head.len(), geo.head_len());

    let mut out = Vec::with_capacity(geo.file_len());
    out.extend_from_slice(&head);
    out.extend_from_slice(&parity);
    out.extend_from_slice(&head);
    out.extend_from_slice(&(head.len() as u64).to_le_bytes());
    out
}

/// A parity file's head, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub geo: Geometry,
    /// The percentage it was made with.
    pub percent: u8,
    /// BLAKE3 of the file it protects.
    pub file_hash: [u8; 32],
    data_hashes: Vec<[u8; HASH]>,
    parity_hashes: Vec<[u8; HASH]>,
}

fn le<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// One copy of the head, at `at`. `None` unless every byte of it checks out.
fn head_at(parity: &[u8], at: usize) -> Option<Head> {
    let fixed = parity.get(at..at.checked_add(FIXED)?)?;
    if &fixed[..8] != MAGIC {
        return None;
    }
    let block = u32::from_le_bytes(le(fixed, 8)?) as usize;
    let stripes = u32::from_le_bytes(le(fixed, 12)?) as usize;
    let m = usize::from(u16::from_le_bytes(le(fixed, 16)?));
    let percent = u16::from_le_bytes(le(fixed, 18)?);
    let len = u64::from_le_bytes(le(fixed, 20)?);
    let file_hash: [u8; 32] = le(fixed, 28)?;
    if !(64..=1 << 20).contains(&block) || !block.is_power_of_two() {
        return None;
    }
    // Nothing below may overflow or ask for more than the parity file can hold.
    let blocks = usize::try_from(len.div_ceil(block as u64)).ok()?;
    if blocks > parity.len() / HASH {
        return None;
    }
    let geo = Geometry {
        len,
        block,
        stripes,
        m,
    };
    let sound = if blocks == 0 {
        stripes == 0 && m == 0
    } else {
        (1..=blocks).contains(&stripes) && m >= 1 && geo.k() + m <= 256
    };
    if !sound || geo.file_len() != parity.len() {
        return None;
    }
    let head = parity.get(at..at.checked_add(geo.head_len())?)?;
    let (body, sum) = head.split_at(head.len() - 32);
    if blake3::hash(body).as_bytes() != sum {
        return None;
    }
    let hashes = body[FIXED..].as_chunks::<HASH>().0;
    let (data_hashes, parity_hashes) = hashes.split_at(blocks);
    Some(Head {
        geo,
        percent: percent.min(u16::from(MAX_PERCENT)) as u8,
        file_hash,
        data_hashes: data_hashes.to_vec(),
        parity_hashes: parity_hashes.to_vec(),
    })
}

/// Read a parity file's head: the copy at the start, or the one at the end.
pub fn head(parity: &[u8]) -> Result<Head, ParityError> {
    if let Some(h) = head_at(parity, 0) {
        return Ok(h);
    }
    // The second copy ends 8 bytes before the end, and its length is written after it.
    let second = (|| {
        let len = usize::try_from(u64::from_le_bytes(le(
            parity,
            parity.len().checked_sub(8)?,
        )?))
        .ok()?;
        head_at(parity, parity.len().checked_sub(8)?.checked_sub(len)?)
    })();
    second.ok_or(ParityError::Unusable(
        "both copies of its head are damaged, or it isn't repair data",
    ))
}

/// A file put right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The file as it was when its parity was made.
    pub bytes: Vec<u8>,
    /// Data blocks that were damaged (0: the file was sound).
    pub bad_data: usize,
    /// Parity blocks that are damaged: the parity file needs writing again.
    pub bad_parity: usize,
    /// The head's first copy was damaged (the second was used).
    pub bad_head: bool,
}

/// Rebuild a file from what is left of it (`damaged`: its bytes as read, with anything
/// that couldn't be read as zeros; too short or too long is fine) and its parity file.
/// The result always matches the checksum of the file the parity was made for.
pub fn restore(damaged: &[u8], parity: &[u8]) -> Result<Restored, ParityError> {
    let h = head(parity)?;
    let bad_head = head_at(parity, 0).is_none();
    let geo = h.geo;
    let (b, g_count, m) = (geo.block, geo.stripes, geo.m);
    let len = usize::try_from(geo.len).map_err(|_| ParityError::Unusable("too large"))?;
    let mut bytes = damaged.to_vec();
    bytes.resize(len, 0);

    let bad: Vec<bool> = (0..geo.blocks())
        .into_par_iter()
        .map(|i| block_hash(&bytes[geo.range(i)]) != h.data_hashes[i])
        .collect();
    let blocks_at = geo.head_len();
    let parity_block = |idx: usize| &parity[blocks_at + idx * b..blocks_at + (idx + 1) * b];
    let parity_ok: Vec<bool> = (0..geo.parity_blocks())
        .into_par_iter()
        .map(|idx| block_hash(parity_block(idx)) == h.parity_hashes[idx])
        .collect();
    let bad_data = bad.iter().filter(|&&x| x).count();
    let bad_parity = parity_ok.iter().filter(|&&x| !x).count();

    for g in 0..g_count {
        // This stripe's columns: block g, g + G, g + 2G…
        let columns: Vec<usize> = (g..geo.blocks()).step_by(g_count).collect();
        let lost: Vec<usize> = (0..columns.len()).filter(|&c| bad[columns[c]]).collect();
        if lost.is_empty() {
            continue;
        }
        let rows: Vec<usize> = (0..m)
            .filter(|&j| parity_ok[geo.parity_index(j, g)])
            .take(lost.len())
            .collect();
        if rows.len() < lost.len() {
            let spare = (0..m)
                .filter(|&j| parity_ok[geo.parity_index(j, g)])
                .count();
            return Err(ParityError::TooMuch {
                lost: lost.len(),
                spare,
                bad_data,
            });
        }
        // For each parity row used: what the lost blocks contributed to it, which is the
        // parity block less everything the surviving blocks put in.
        let e = lost.len();
        let mut rhs = vec![0u8; e * b];
        rhs.par_chunks_mut(b).zip(&rows).for_each(|(s, &j)| {
            s.copy_from_slice(parity_block(geo.parity_index(j, g)));
            for (c, &i) in columns.iter().enumerate() {
                if !bad[i] {
                    let src = &bytes[geo.range(i)];
                    gf256::mul_add(&mut s[..src.len()], coefficient(j, c), src);
                }
            }
        });
        // Solve  A·x = rhs  for the lost blocks x, with A[r][q] = C[rows[r]][lost[q]].
        let a: Vec<u8> = rows
            .iter()
            .flat_map(|&j| lost.iter().map(move |&c| coefficient(j, c)))
            .collect();
        let a_inv = gf256::invert(a, e).ok_or(ParityError::Unusable("no solution"))?;
        let rebuilt: Vec<Vec<u8>> = (0..e)
            .into_par_iter()
            .map(|q| {
                let mut x = vec![0u8; b];
                for r in 0..e {
                    gf256::mul_add(&mut x, a_inv[q * e + r], &rhs[r * b..(r + 1) * b]);
                }
                x
            })
            .collect();
        for (q, x) in rebuilt.iter().enumerate() {
            let range = geo.range(columns[lost[q]]);
            let n = range.len();
            bytes[range].copy_from_slice(&x[..n]);
        }
    }
    if *blake3::hash(&bytes).as_bytes() != h.file_hash {
        return Err(ParityError::Mismatch);
    }
    Ok(Restored {
        bytes,
        bad_data,
        bad_parity,
        bad_head,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repeatable noise.
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }

    /// Spoil whole blocks (every byte changed).
    fn spoil(data: &mut [u8], blocks: &[usize]) {
        for &i in blocks {
            let end = ((i + 1) * BLOCK).min(data.len());
            data[i * BLOCK..end].iter_mut().for_each(|x| *x ^= 0xa5);
        }
    }

    #[test]
    fn layouts() {
        // Stripes never exceed 256 blocks, and the parity is about the share asked for.
        for percent in [1u8, 2, 5, 10, 20, 33, 50] {
            for len in [
                1u64,
                4096,
                4097,
                100_000,
                1 << 20,
                16 << 20,
                (24 << 20) + 123,
                1 << 30,
            ] {
                let g = Geometry::plan(len, percent);
                assert!(g.k() + g.m <= 256, "{percent}% of {len}: {g:?}");
                assert!(g.m >= 2);
                assert!(g.stripes * g.k() >= g.blocks());
                let share = g.parity_blocks() as f64 / g.blocks() as f64;
                if g.blocks() >= 1_000 {
                    let asked = f64::from(percent) / 100.0;
                    assert!(
                        share >= asked && share < asked * 1.2 + 0.005,
                        "{percent}% of {len}: {share}"
                    );
                }
            }
        }
        let empty = Geometry::plan(0, 5);
        assert_eq!((empty.blocks(), empty.parity_blocks()), (0, 0));
        // A 16 MiB pack at 5%: 17 stripes of up to 241 blocks, 13 parity blocks each.
        let pack = Geometry::plan(16 << 20, 5);
        assert_eq!(
            (pack.blocks(), pack.stripes, pack.k(), pack.m),
            (4096, 17, 241, 13)
        );
        // The Cauchy points never collide, at the limits of every layout.
        for k in 1..=254usize {
            let m = 256 - k;
            let _ = coefficient(m - 1, k - 1);
        }
    }

    #[test]
    fn sound_files_come_back_as_they_are() {
        let mut noise = Noise(1);
        for len in [0usize, 1, 4095, 4096, 4097, 50_000, 300_000] {
            let data = noise.bytes(len);
            let parity = encode(&data, 5);
            assert_eq!(parity.len(), Geometry::plan(len as u64, 5).file_len());
            let r = restore(&data, &parity).unwrap();
            assert_eq!(
                (r.bytes == data, r.bad_data, r.bad_parity, r.bad_head),
                (true, 0, 0, false),
                "{len}"
            );
        }
    }

    #[test]
    fn any_m_blocks_of_a_stripe_are_rebuilt_and_one_more_is_not() {
        let mut noise = Noise(2);
        // One stripe: 100 blocks, 10% → 10 parity blocks.
        let data = noise.bytes(100 * BLOCK - 700);
        let parity = encode(&data, 10);
        let geo = head(&parity).unwrap().geo;
        assert_eq!((geo.stripes, geo.m), (1, 10));
        for round in 0..40 {
            // Ten blocks, anywhere (the last, short block among them now and then).
            let mut lost: Vec<usize> = Vec::new();
            while lost.len() < 10 {
                let i = if round % 4 == 0 && lost.is_empty() {
                    99
                } else {
                    noise.below(100)
                };
                if !lost.contains(&i) {
                    lost.push(i);
                }
            }
            let mut damaged = data.clone();
            spoil(&mut damaged, &lost);
            let r = restore(&damaged, &parity).unwrap();
            assert_eq!((r.bytes == data, r.bad_data), (true, 10), "round {round}");
            // An eleventh: not repairable, and it says so rather than guessing.
            let extra = (0..100).find(|i| !lost.contains(i)).unwrap();
            spoil(&mut damaged, &[extra]);
            assert_eq!(
                restore(&damaged, &parity),
                Err(ParityError::TooMuch {
                    lost: 11,
                    spare: 10,
                    bad_data: 11
                })
            );
        }
    }

    #[test]
    fn damage_of_every_kind() {
        let mut noise = Noise(3);
        let data = noise.bytes(3_000_000); // 733 blocks: 4 stripes at 5%
        let parity = encode(&data, 5);
        let geo = head(&parity).unwrap().geo;
        assert_eq!((geo.stripes, geo.m), (4, 10));

        // One bit.
        let mut d = data.clone();
        d[1_234_567] ^= 0x10;
        let r = restore(&d, &parity).unwrap();
        assert_eq!((r.bytes == data, r.bad_data), (true, 1));

        // A run of m·G blocks (160 KiB), anywhere: spread over the stripes, m to each.
        for start in [0usize, 1, 333, 733 - 40] {
            let mut d = data.clone();
            spoil(&mut d, &(start..start + 40).collect::<Vec<_>>());
            let r = restore(&d, &parity).unwrap();
            assert_eq!((r.bytes == data, r.bad_data), (true, 40), "run at {start}");
        }
        // One block more than that is too much.
        let mut d = data.clone();
        spoil(&mut d, &(100..141).collect::<Vec<_>>());
        assert!(matches!(
            restore(&d, &parity),
            Err(ParityError::TooMuch {
                lost: 11,
                spare: 10,
                ..
            })
        ));

        // The end cut off (a truncated file), and junk added: the length is restored.
        let r = restore(&data[..data.len() - 100_000], &parity).unwrap();
        assert_eq!((r.bytes == data, r.bad_data), (true, 25));
        let mut longer = data.clone();
        longer.extend_from_slice(b"junk");
        assert!(restore(&longer, &parity).unwrap().bytes == data);

        // Damage in the parity file as well: its blocks are checked like the data's.
        let blocks_at = geo.head_len();
        let mut p = parity.clone();
        for idx in [0usize, 5, 17] {
            p[blocks_at + idx * BLOCK + 9] ^= 1;
        }
        let mut d = data.clone();
        spoil(&mut d, &[7, 8, 9, 10, 400]);
        let r = restore(&d, &p).unwrap();
        assert_eq!((r.bytes == data, r.bad_data, r.bad_parity), (true, 5, 3));

        // Either copy of the head may be damaged, not both.
        let mut p = parity.clone();
        p[40] ^= 0xff;
        let r = restore(&d, &p).unwrap();
        assert!(r.bytes == data && r.bad_head);
        let mut p = parity.clone();
        let n = p.len();
        p[n - 20] ^= 0xff; // in the second copy
        p[n - 3] ^= 0xff; // and its length
        assert!(restore(&d, &p).unwrap().bytes == data);
        let mut p = parity.clone();
        p[40] ^= 0xff;
        p[n - 200] ^= 0xff;
        assert!(matches!(restore(&d, &p), Err(ParityError::Unusable(_))));

        // Parity for another file never "repairs" this one into it.
        let other = encode(&noise.bytes(3_000_000), 5);
        assert!(matches!(
            restore(&data, &other),
            Err(ParityError::TooMuch { .. })
        ));
    }

    #[test]
    fn scattered_damage_at_random() {
        // Random files, shares and damage: whenever no stripe loses more than m blocks
        // the file comes back exactly; otherwise it is refused, never wrong.
        let mut noise = Noise(4);
        let (mut repaired, mut refused) = (0, 0);
        for round in 0..60 {
            let len = 1 + noise.below(1_500_000);
            let percent = [1u8, 3, 5, 10, 25, 50][noise.below(6)];
            let data = noise.bytes(len);
            let parity = encode(&data, percent);
            let geo = head(&parity).unwrap().geo;
            let hits = noise.below(geo.parity_blocks() * 2 + 2);
            let mut lost: Vec<usize> = (0..hits).map(|_| noise.below(geo.blocks())).collect();
            lost.sort_unstable();
            lost.dedup();
            let mut d = data.clone();
            spoil(&mut d, &lost);
            let worst = (0..geo.stripes)
                .map(|g| lost.iter().filter(|&&i| i % geo.stripes == g).count())
                .max()
                .unwrap_or(0);
            match restore(&d, &parity) {
                Ok(r) => {
                    assert!(worst <= geo.m, "round {round}");
                    assert!(r.bytes == data && r.bad_data == lost.len(), "round {round}");
                    repaired += 1;
                }
                Err(e) => {
                    assert!(worst > geo.m, "round {round}: {e}");
                    refused += 1;
                }
            }
        }
        assert!(repaired > 10 && refused > 5, "{repaired} {refused}");
    }

    /// How fast: `cargo test --release -p flux-vault --lib speed -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a test"]
    fn speed() {
        let data = Noise(9).bytes(16 << 20);
        for percent in [2u8, 5, 10, 20] {
            let t = std::time::Instant::now();
            let parity = encode(&data, percent);
            let enc = t.elapsed();
            let mut d = data.clone();
            spoil(&mut d, &(1000..1200).collect::<Vec<_>>());
            let t = std::time::Instant::now();
            let ok = restore(&d, &parity).is_ok();
            println!(
                "{percent}%: encode 16 MiB in {enc:?} ({:.0} MiB/s), repair 200 blocks in {:?} ({ok})",
                16.0 / enc.as_secs_f64(),
                t.elapsed()
            );
        }
    }

    #[test]
    fn junk_is_not_repair_data() {
        let mut noise = Noise(5);
        assert!(head(&[]).is_err());
        assert!(head(b"FLUXPAR1").is_err());
        for _ in 0..200 {
            let n = noise.below(600);
            let mut junk = noise.bytes(n);
            if n >= 8 && noise.below(2) == 0 {
                junk[..8].copy_from_slice(MAGIC);
            }
            assert!(head(&junk).is_err());
        }
        // A real head with its fields changed one at a time: caught by its checksum, and
        // never a panic or a huge allocation.
        let parity = encode(&noise.bytes(20_000), 5);
        for at in 0..FIXED {
            for bit in 0..8 {
                let mut p = parity.clone();
                p[at] ^= 1 << bit;
                let n = p.len();
                p[n - 9] ^= 1; // and the other copy
                assert!(head(&p).is_err(), "byte {at} bit {bit}");
            }
        }
    }
}
