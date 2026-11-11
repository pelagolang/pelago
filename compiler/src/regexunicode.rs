//! Unicode for `@regex`: sets of code points and their UTF-8 byte sequences (`(?u)` mode).
//!
//! The matcher stays a byte machine. A set of code points becomes an alternation of byte-range sequences (the
//! standard UTF-8 range split), so one code point costs one pass over its bytes and the match stays linear. The
//! sets for `\w`, `\d` and `\s` are Python's own (generated, see `regexunicode_tables.rs`); case folding uses the
//! simple case mappings of the Rust standard library.

/// A sorted list of non-overlapping inclusive ranges of code points.
pub type Set = Vec<(u32, u32)>;

const MAX: u32 = 0x10FFFF;

/// Sorts and merges ranges.
pub fn norm(mut v: Set) -> Set {
    v.sort_unstable();
    let mut out: Set = Vec::new();
    for (lo, hi) in v {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// Every code point (surrogates are not code points of text) that is not in the set.
pub fn complement(v: &Set) -> Set {
    let v = norm(v.clone());
    let mut out = Vec::new();
    let mut next = 0u32;
    for (lo, hi) in v {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi + 1;
    }
    if next <= MAX {
        out.push((next, MAX));
    }
    without_surrogates(out)
}

fn without_surrogates(v: Set) -> Set {
    let mut out = Vec::new();
    for (lo, hi) in v {
        if hi < 0xD800 || lo > 0xDFFF {
            out.push((lo, hi));
            continue;
        }
        if lo < 0xD800 {
            out.push((lo, 0xD7FF));
        }
        if hi > 0xDFFF {
            out.push((0xE000, hi));
        }
    }
    out
}

/// `\d`: the decimal digits, as Python's `re` sees them.
pub fn digits() -> Set {
    crate::regexunicode_tables::DIGITS.to_vec()
}

/// `\w`: letters, numbers and the underscore, as Python's `re` sees them.
pub fn word() -> Set {
    crate::regexunicode_tables::WORD.to_vec()
}

/// `\s`: white space, as Python's `re` sees it.
pub fn space() -> Set {
    crate::regexunicode_tables::SPACE.to_vec()
}

/// The character an iterator holds when it holds exactly one.
fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let x = it.next()?;
    it.next().is_none().then_some(x)
}

/// Adds the simple upper and lower case of every member: `(?i)`.
pub fn fold(v: &Set) -> Set {
    let mut out = v.clone();
    for &(lo, hi) in v {
        for cp in lo..=hi {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            for x in [single(c.to_lowercase()), single(c.to_uppercase())]
                .into_iter()
                .flatten()
            {
                if x != c {
                    out.push((x as u32, x as u32));
                }
            }
        }
    }
    norm(out)
}

/// The byte sequences of a range of code points, as lists of inclusive byte ranges (one range per byte).
pub fn sequences(lo: u32, hi: u32, out: &mut Vec<Vec<(u8, u8)>>) {
    if lo > hi {
        return;
    }
    // Text has no surrogates.
    if lo <= 0xDFFF && hi >= 0xD800 {
        if lo < 0xD800 {
            sequences(lo, 0xD7FF, out);
        }
        if hi > 0xDFFF {
            sequences(0xE000, hi, out);
        }
        return;
    }
    // One length at a time.
    for max in [0x7F, 0x7FF, 0xFFFF] {
        if lo <= max && max < hi {
            sequences(lo, max, out);
            sequences(max + 1, hi, out);
            return;
        }
    }
    if hi <= 0x7F {
        out.push(vec![(lo as u8, hi as u8)]);
        return;
    }
    // Split until every continuation byte of the range runs over its whole span.
    for i in 1..4 {
        let m = (1u32 << (6 * i)) - 1;
        if (lo & !m) != (hi & !m) {
            if (lo & m) != 0 {
                sequences(lo, lo | m, out);
                sequences((lo | m) + 1, hi, out);
                return;
            }
            if (hi & m) != m {
                sequences(lo, (hi & !m) - 1, out);
                sequences(hi & !m, hi, out);
                return;
            }
        }
    }
    let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
    let (Some(x), Some(y)) = (char::from_u32(lo), char::from_u32(hi)) else {
        return;
    };
    let (x, y) = (
        x.encode_utf8(&mut a).as_bytes().to_vec(),
        y.encode_utf8(&mut b).as_bytes().to_vec(),
    );
    out.push(x.iter().zip(&y).map(|(p, q)| (*p, *q)).collect());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(set: &Set, c: char) -> bool {
        let mut seqs = Vec::new();
        for &(lo, hi) in set {
            sequences(lo, hi, &mut seqs);
        }
        let mut buf = [0u8; 4];
        let bytes = c.encode_utf8(&mut buf).as_bytes();
        seqs.iter().any(|s| {
            s.len() == bytes.len() && s.iter().zip(bytes).all(|((lo, hi), b)| lo <= b && b <= hi)
        })
    }

    #[test]
    fn the_byte_sequences_of_a_set_match_exactly_its_code_points() {
        let set = norm(vec![
            (0x41, 0x5A),
            (0xE0, 0xFF),
            (0x3B1, 0x3C9),
            (0x4E00, 0x9FFF),
            (0x1F600, 0x1F64F),
        ]);
        for cp in (0..0x20000u32).chain(0x4DF0..0x4E10).chain(0x9FF0..0xA010) {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let inside = set.iter().any(|&(lo, hi)| lo <= cp && cp <= hi);
            assert_eq!(matches(&set, c), inside, "U+{cp:04X}");
        }
    }

    #[test]
    fn the_complement_leaves_out_exactly_the_set() {
        let set = norm(vec![(0x61, 0x7A)]);
        let c = complement(&set);
        assert!(c.iter().all(|&(lo, hi)| hi < 0x61 || lo > 0x7A));
        assert_eq!(complement(&c), set);
    }

    #[test]
    fn folding_adds_the_other_case() {
        let f = fold(&norm(vec![(0xE9, 0xE9)]));
        assert_eq!(f, vec![(0xC9, 0xC9), (0xE9, 0xE9)]);
    }
}
