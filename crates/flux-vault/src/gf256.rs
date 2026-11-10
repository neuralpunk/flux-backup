//! Arithmetic in GF(2⁸), the field with 256 elements that Reed–Solomon repair data is
//! computed in (see [`crate::parity`]).
//!
//! A byte is a polynomial over GF(2) of degree under 8 (bit `i` is the coefficient of
//! `xⁱ`). Addition is XOR. Multiplication is polynomial multiplication modulo
//! `x⁸ + x⁴ + x³ + x² + 1` (0x11d), the polynomial RAID-6 and PAR2 use; `x` (the byte 2)
//! generates the 255 non-zero elements, so every non-zero byte is `2ᵉ` for one `e` in
//! 0..255, and products are sums of exponents.

use std::sync::LazyLock;

/// The reducing polynomial, without its `x⁸` term.
const POLY: u8 = 0x1d;

/// `EXP[e] = 2ᵉ` for e in 0..510 (twice round, so two logarithms add without a modulo),
/// and `LOG[2ᵉ] = e`.
const fn tables() -> ([u8; 510], [u8; 256]) {
    let mut exp = [0u8; 510];
    let mut log = [0u8; 256];
    let mut x = 1u8;
    let mut e = 0;
    while e < 255 {
        exp[e] = x;
        exp[e + 255] = x;
        log[x as usize] = e as u8;
        // Times x: shift, and subtract the polynomial if x⁸ appeared.
        x = (x << 1) ^ (if x & 0x80 != 0 { POLY } else { 0 });
        e += 1;
    }
    (exp, log)
}

const TABLES: ([u8; 510], [u8; 256]) = tables();
const EXP: [u8; 510] = TABLES.0;
const LOG: [u8; 256] = TABLES.1;

pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + LOG[b as usize] as usize]
    }
}

/// `1 / a`. There is no inverse of zero: callers never ask (debug builds check).
pub fn inv(a: u8) -> u8 {
    debug_assert!(a != 0, "no inverse of zero");
    EXP[255 - LOG[a as usize] as usize]
}

/// Every product: `PRODUCTS[c][x] = c·x`. 64 KiB, built on first use.
static PRODUCTS: LazyLock<Box<[[u8; 256]; 256]>> = LazyLock::new(|| {
    let mut t = vec![[0u8; 256]; 256];
    for (c, row) in t.iter_mut().enumerate() {
        for (x, p) in row.iter_mut().enumerate() {
            *p = mul(c as u8, x as u8);
        }
    }
    t.into_boxed_slice().try_into().expect("256 rows")
});

/// `out ← out + c·src`, byte by byte (the lengths must match).
pub fn mul_add(out: &mut [u8], c: u8, src: &[u8]) {
    debug_assert_eq!(out.len(), src.len());
    match c {
        0 => {}
        1 => out.iter_mut().zip(src).for_each(|(o, s)| *o ^= s),
        _ => {
            let row = &PRODUCTS[c as usize];
            out.iter_mut()
                .zip(src)
                .for_each(|(o, s)| *o ^= row[*s as usize]);
        }
    }
}

/// Invert a square matrix (row-major, `n × n`) by Gauss–Jordan elimination. `None` if it
/// has no inverse.
pub fn invert(mut a: Vec<u8>, n: usize) -> Option<Vec<u8>> {
    debug_assert_eq!(a.len(), n * n);
    let mut inv_m = vec![0u8; n * n];
    for i in 0..n {
        inv_m[i * n + i] = 1;
    }
    for col in 0..n {
        let pivot = (col..n).find(|&r| a[r * n + col] != 0)?;
        if pivot != col {
            for c in 0..n {
                a.swap(pivot * n + c, col * n + c);
                inv_m.swap(pivot * n + c, col * n + c);
            }
        }
        let scale = inv(a[col * n + col]);
        for c in 0..n {
            a[col * n + c] = mul(a[col * n + c], scale);
            inv_m[col * n + c] = mul(inv_m[col * n + c], scale);
        }
        for r in 0..n {
            let f = a[r * n + col];
            if r != col && f != 0 {
                for c in 0..n {
                    a[r * n + c] ^= mul(f, a[col * n + c]);
                    inv_m[r * n + c] ^= mul(f, inv_m[col * n + c]);
                }
            }
        }
    }
    Some(inv_m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Multiplication from the definition: polynomials over GF(2), reduced by 0x11d.
    fn slow_mul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0;
        while b != 0 {
            if b & 1 != 0 {
                p ^= a;
            }
            a = (a << 1) ^ (if a & 0x80 != 0 { POLY } else { 0 });
            b >>= 1;
        }
        p
    }

    #[test]
    fn it_is_a_field() {
        // 2 generates every non-zero element once.
        let mut seen = [false; 256];
        for e in 0..255 {
            assert!(!seen[EXP[e] as usize]);
            seen[EXP[e] as usize] = true;
        }
        assert!(!seen[0] && seen[1..].iter().all(|&s| s));
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                // The tables agree with the definition, for all 65,536 products.
                assert_eq!(mul(a, b), slow_mul(a, b), "{a}·{b}");
                assert_eq!(mul(a, b), mul(b, a));
            }
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(a, 0), 0);
            if a != 0 {
                assert_eq!(mul(a, inv(a)), 1, "{a}");
            }
        }
        // Associative and distributive, over a spread of triples.
        for a in (0..=255u8).step_by(7) {
            for b in (0..=255u8).step_by(11) {
                for c in (0..=255u8).step_by(13) {
                    assert_eq!(mul(mul(a, b), c), mul(a, mul(b, c)));
                    assert_eq!(mul(a, b ^ c), mul(a, b) ^ mul(a, c));
                }
            }
        }
    }

    #[test]
    fn rows_and_matrices() {
        let src: Vec<u8> = (0..=255).collect();
        for c in [0u8, 1, 2, 0x1d, 0x8e, 255] {
            let mut out = vec![0x5a; 256];
            mul_add(&mut out, c, &src);
            for (x, o) in out.iter().enumerate() {
                assert_eq!(*o, 0x5a ^ mul(c, x as u8));
            }
        }
        // A matrix times its inverse is the identity.
        let n = 5;
        let a: Vec<u8> = (0..n * n)
            .map(|i| inv(((i / n) as u8) ^ (200 + (i % n) as u8)))
            .collect();
        let b = invert(a.clone(), n).expect("a Cauchy matrix has an inverse");
        for r in 0..n {
            for c in 0..n {
                let dot = (0..n).fold(0, |s, k| s ^ mul(a[r * n + k], b[k * n + c]));
                assert_eq!(dot, u8::from(r == c));
            }
        }
        // Two equal rows: no inverse.
        assert_eq!(invert(vec![1, 2, 1, 2], 2), None);
    }
}
