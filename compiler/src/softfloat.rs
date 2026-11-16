//! Software IEEE 754 numbers at compile time: turning the text of a literal into the bits of a `float16`,
//! `float128` or `float256` (and any other width), rounded correctly (to nearest, ties to even).
//!
//! The run-time arithmetic is written in the language itself (`compiler/core/core/softfloat.pel`); this file
//! is only what the compiler needs to turn `0.1` into the right bit pattern. The numbers are small big
//! integers: `Vec<u64>`, least significant word first.

type Big = Vec<u64>;

/// A binary format: how many exponent bits and fraction bits it has (the sign bit comes on top).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub eb: u32,
    pub mb: u32,
}

impl Format {
    pub const F16: Format = Format { eb: 5, mb: 10 };
    pub const F128: Format = Format { eb: 15, mb: 112 };
    pub const F256: Format = Format { eb: 19, mb: 236 };

    /// 64-bit words a value takes in the working representation (16 bits fit one).
    pub fn words(self) -> usize {
        ((1 + self.eb + self.mb) as usize).div_ceil(64)
    }
}

fn trim(a: &mut Big) {
    while a.len() > 1 && a.last() == Some(&0) {
        a.pop();
    }
}

fn bit_len(a: &Big) -> u64 {
    for (i, w) in a.iter().enumerate().rev() {
        if *w != 0 {
            return i as u64 * 64 + 64 - w.leading_zeros() as u64;
        }
    }
    0
}

fn is_zero(a: &Big) -> bool {
    a.iter().all(|w| *w == 0)
}

fn mul_small(a: &mut Big, m: u64) {
    let mut carry = 0u128;
    for w in a.iter_mut() {
        let v = *w as u128 * m as u128 + carry;
        *w = v as u64;
        carry = v >> 64;
    }
    if carry != 0 {
        a.push(carry as u64);
    }
}

fn add_small(a: &mut Big, d: u64) {
    let mut carry = d;
    for w in a.iter_mut() {
        let (s, c) = w.overflowing_add(carry);
        *w = s;
        carry = c as u64;
        if carry == 0 {
            return;
        }
    }
    if carry != 0 {
        a.push(carry);
    }
}

fn shl(a: &Big, bits: u64) -> Big {
    let (ws, bs) = ((bits / 64) as usize, (bits % 64) as u32);
    let mut out = vec![0u64; ws];
    let mut carry = 0u64;
    for w in a {
        if bs == 0 {
            out.push(*w);
        } else {
            out.push((w << bs) | carry);
            carry = w >> (64 - bs);
        }
    }
    if carry != 0 {
        out.push(carry);
    }
    trim(&mut out);
    out
}

/// The bits above `bits` (a right shift), and whether any bit shifted out was set.
fn shr_sticky(a: &Big, bits: u64) -> (Big, bool) {
    let total = a.len() as u64 * 64;
    if bits >= total {
        return (vec![0], !is_zero(a));
    }
    let mut lost = false;
    let (ws, bs) = ((bits / 64) as usize, (bits % 64) as u32);
    for w in &a[..ws] {
        lost |= *w != 0;
    }
    if bs > 0 {
        lost |= a[ws] & ((1u64 << bs) - 1) != 0;
    }
    let mut out = Vec::new();
    for i in ws..a.len() {
        let lo = a[i] >> bs;
        let hi = if bs > 0 && i + 1 < a.len() {
            a[i + 1] << (64 - bs)
        } else {
            0
        };
        out.push(lo | hi);
    }
    trim(&mut out);
    (out, lost)
}

fn cmp(a: &Big, b: &Big) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in (0..n).rev() {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

fn sub_in(a: &mut Big, b: &Big) {
    let mut borrow = false;
    for (i, x) in a.iter_mut().enumerate() {
        let y = b.get(i).copied().unwrap_or(0);
        let (d1, b1) = x.overflowing_sub(y);
        let (d2, b2) = d1.overflowing_sub(borrow as u64);
        *x = d2;
        borrow = b1 || b2;
    }
}

fn add_big(a: &Big, b: &Big) -> Big {
    let mut out = Vec::new();
    let mut carry = false;
    for i in 0..a.len().max(b.len()) {
        let (s1, c1) = a
            .get(i)
            .copied()
            .unwrap_or(0)
            .overflowing_add(b.get(i).copied().unwrap_or(0));
        let (s2, c2) = s1.overflowing_add(carry as u64);
        out.push(s2);
        carry = c1 || c2;
    }
    if carry {
        out.push(1);
    }
    out
}

fn bit(a: &Big, i: u64) -> bool {
    a.get((i / 64) as usize)
        .is_some_and(|w| w >> (i % 64) & 1 == 1)
}

/// `n / d` limited to a quotient of `qbits` bits (the caller made `n` the right size): quotient and "remainder
/// is not zero".
fn div_bits(n: &Big, d: &Big, qbits: u64) -> (Big, bool) {
    let mut r = n.clone();
    let mut q = vec![0u64; (qbits as usize).div_ceil(64) + 1];
    for i in (0..=qbits).rev() {
        let ds = shl(d, i);
        if cmp(&r, &ds) != std::cmp::Ordering::Less {
            sub_in(&mut r, &ds);
            q[(i / 64) as usize] |= 1 << (i % 64);
        }
    }
    trim(&mut q);
    (q, !is_zero(&r))
}

fn pow10(k: u64) -> Big {
    let mut v = vec![1u64];
    for _ in 0..k / 18 {
        mul_small(&mut v, 1_000_000_000_000_000_000);
    }
    for _ in 0..k % 18 {
        mul_small(&mut v, 10);
    }
    v
}

/// Rounds `m * 2^e` (and a sticky bit for anything below) to the format; returns the words.
pub fn pack(neg: bool, e: i64, m: &Big, sticky: bool, f: Format) -> Vec<u64> {
    let n = f.words();
    let mut out = vec![0u64; n];
    let (eb, mb) = (f.eb as i64, f.mb as i64);
    let set_bit = |v: &mut Vec<u64>, i: i64| v[(i / 64) as usize] |= 1 << (i % 64);
    let b = bit_len(m) as i64;
    let inf = |out: &mut Vec<u64>| {
        for k in 0..eb {
            set_bit(out, mb + k);
        }
        if neg {
            set_bit(out, eb + mb);
        }
    };
    if b == 0 {
        if neg {
            set_bit(&mut out, eb + mb);
        }
        return out;
    }
    let bias = (1i64 << (eb - 1)) - 1;
    let emin = 1 - bias;
    let top = e + b - 1;
    if top > bias {
        inf(&mut out);
        return out;
    }
    let sub = top < emin;
    let shift = if sub { emin - mb - e } else { b - (mb + 1) };
    let (mut kept, guard, lost) = if shift > 0 {
        let (k, lost_all) = shr_sticky(m, shift as u64);
        let g = bit(m, shift as u64 - 1);
        // `lost_all` includes the guard bit itself.
        let below = if shift >= 1 {
            let (_, l) = shr_sticky(m, shift as u64 - 1);
            l
        } else {
            false
        };
        let _ = lost_all;
        (k, g, below)
    } else {
        (shl(m, (-shift) as u64), false, false)
    };
    if guard && (sticky || lost || bit(&kept, 0)) {
        add_small(&mut kept, 1);
    }
    let raw = if sub {
        kept
    } else {
        let hi = shl(&vec![(top + bias - 1) as u64], mb as u64);
        add_big(&hi, &kept)
    };
    let mut inf_pat = vec![0u64; n];
    for k in 0..eb {
        set_bit(&mut inf_pat, mb + k);
    }
    if cmp(&raw, &inf_pat) != std::cmp::Ordering::Less {
        inf(&mut out);
        return out;
    }
    for (i, w) in raw.iter().enumerate() {
        if i < n {
            out[i] = *w;
        }
    }
    if neg {
        set_bit(&mut out, eb + mb);
    }
    out
}

/// The bits of a decimal literal (`12`, `0.1`, `1_000.5e-3`, `2e30`) in the format, or `None` when the text is not one.
pub fn parse(text: &str, f: Format) -> Option<Vec<u64>> {
    let t: String = text.chars().filter(|c| *c != '_').collect();
    let (mant, exp) = match t.find(['e', 'E']) {
        Some(i) => (&t[..i], t[i + 1..].parse::<i64>().ok()?),
        None => (&t[..], 0),
    };
    let mut n: Big = vec![0];
    let mut frac_digits = 0i64;
    let mut seen_dot = false;
    for c in mant.chars() {
        if c == '.' {
            if seen_dot {
                return None;
            }
            seen_dot = true;
            continue;
        }
        let d = c.to_digit(10)? as u64;
        mul_small(&mut n, 10);
        add_small(&mut n, d);
        if seen_dot {
            frac_digits += 1;
        }
    }
    let k = (exp - frac_digits).clamp(-300_000, 300_000);
    let p = f.mb as u64 + 1;
    if is_zero(&n) {
        return Some(pack(false, 0, &vec![0], false, f));
    }
    if k >= 0 {
        let mut x = n;
        let big = pow10(k as u64);
        // x * 10^k
        let mut acc: Big = vec![0];
        for (i, w) in big.iter().enumerate() {
            let mut part = x.clone();
            mul_small(&mut part, *w);
            acc = add_big(&acc, &shl(&part, 64 * i as u64));
        }
        x = acc;
        trim(&mut x);
        Some(pack(false, 0, &x, false, f))
    } else {
        let d = pow10((-k) as u64);
        // Scale the numerator so the quotient has p + 3 bits or more.
        let s = (p + 3 + bit_len(&d)).saturating_sub(bit_len(&n)) as u64;
        let num = shl(&n, s);
        // The quotient has `p + 4` bits when the numerator was scaled up; a long digit string is already larger.
        let qbits = (p + 4).max((bit_len(&num) + 1).saturating_sub(bit_len(&d)));
        let (q, sticky) = div_bits(&num, &d, qbits);
        Some(pack(false, -(s as i64), &q, sticky, f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words of a hex number, least significant first.
    fn words(hex: &str, n: usize) -> Vec<u64> {
        let h = hex.trim_start_matches("0x");
        let mut out = vec![0u64; n];
        let bytes: Vec<char> = h.chars().collect();
        for (i, chunk) in bytes.rchunks(16).enumerate() {
            let s: String = chunk.iter().collect();
            out[i] = u64::from_str_radix(&s, 16).unwrap();
        }
        out
    }

    #[test]
    fn a_long_digit_string_is_not_cut_short() {
        // 59 digits with a fraction: the quotient is far longer than the format's significand.
        // Expected value computed independently with exact rational arithmetic.
        let got = parse(
            "589351201404482111405296861187477616363576688419074327694529.0",
            Format::F128,
        )
        .unwrap();
        assert_eq!(got, words("0x40c5778e6693db7b60d226b21086f734", 2));
    }

    #[test]
    fn literals_round_to_nearest_even() {
        // Expected bit patterns computed independently with exact rational arithmetic.
        let cases: [(&str, &str, &str, &str); 11] = [
            (
                "0.1",
                "0x3ffb999999999999999999999999999a",
                "0x2e66",
                "0x3fffb9999999999999999999999999999999999999999999999999999999999a",
            ),
            (
                "1",
                "0x3fff0000000000000000000000000000",
                "0x3c00",
                "0x3ffff00000000000000000000000000000000000000000000000000000000000",
            ),
            (
                "3.14159265358979323846264338327950288",
                "0x4000921fb54442d18469898cc51701b8",
                "0x4248",
                "0x40000921fb54442d18469898cc51701b836d83581f1c2243aef09852d2f9ef9b",
            ),
            (
                "1e-4940",
                "0x000cc64f1cc4376f7da08f39",
                "0x0",
                "0x3bfe498c9e39886edefb411e729aa1d8862640dcda636833633573b07e4e8118",
            ),
            (
                "1e4932",
                "0x7ffeae596552b8fded99d037e3d04b75",
                "0x7c00",
                "0x43ffeae596552b8fded99d037e3d04b74ea2b85fd6a43ad96bb9ae2260063fde",
            ),
            (
                "123456789.123456789",
                "0x4019d6f34547e6b74dce58d7cc490820",
                "0x7c00",
                "0x40019d6f34547e6b74dce58d7cc49082079b6e9fb4aa85e9d331af94468d46ea",
            ),
            (
                "5e-324",
                "0x3bcd03132b9cf541c364e687dfd7a328",
                "0x0",
                "0x3fbcd03132b9cf541c364e687dfd7a32813279936f65cdd6e18d8c74e042208d",
            ),
            (
                "0.5",
                "0x3ffe0000000000000000000000000000",
                "0x3800",
                "0x3fffe00000000000000000000000000000000000000000000000000000000000",
            ),
            (
                "65504",
                "0x400effc0000000000000000000000000",
                "0x7bff",
                "0x4000effc00000000000000000000000000000000000000000000000000000000",
            ),
            (
                "1e-7",
                "0x3fe7ad7f29abcaf485787a6520ec08d2",
                "0x2",
                "0x3ffe7ad7f29abcaf485787a6520ec08d23699194119a5c37387b71906614310f",
            ),
            (
                "2.5e30",
                "0x4063f8def8808b02452c9a0000000000",
                "0x7c00",
                "0x40063f8def8808b02452c9a00000000000000000000000000000000000000000",
            ),
        ];
        for (text, h128, h16, h256) in cases {
            assert_eq!(
                parse(text, Format::F128).unwrap(),
                words(h128, 2),
                "{text} as float128"
            );
            assert_eq!(
                parse(text, Format::F16).unwrap(),
                words(h16, 1),
                "{text} as float16"
            );
            assert_eq!(
                parse(text, Format::F256).unwrap(),
                words(h256, 4),
                "{text} as float256"
            );
        }
    }
}
