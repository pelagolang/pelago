//! Unicode for `@regex`: sets of code points and their UTF-8 byte sequences (`(?u)` mode).
//!
//! The matcher stays a byte machine. A set of code points becomes an alternation of byte-range sequences (the
//! standard UTF-8 range split), so one code point costs one pass over its bytes and the match stays linear. The
//! sets for `\w`, `\d` and `\s`, the case equivalences and the general categories are Python's own data (generated, see
//! `regexunicode_tables.rs`), so the engine and its test oracle agree.

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

/// Whether the code point is in the (sorted, merged) set.
fn has(v: &Set, cp: u32) -> bool {
    let i = v.partition_point(|&(_, hi)| hi < cp);
    i < v.len() && v[i].0 <= cp
}

/// Adds every code point Python's `re` treats as the same letter as a member when it ignores case: all code points
/// with the same lower case, and the extra equivalences of `re._casefix` (`s` and `\u{17f}`, `\u{3c3}` and `\u{3c2}`,
/// the Kelvin sign and `k`, ...). Tables come from Python itself, so the two engines agree on which letters match.
pub fn fold(v: &Set) -> Set {
    use crate::regexunicode_tables::{EXTRA, LOWER};
    use std::collections::HashMap;
    let v = norm(v.clone());
    let mut same: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(cp, low) in LOWER {
        same.entry(low).or_default().push(cp);
    }
    let mut extra: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(k, other) in EXTRA {
        extra.entry(k).or_default().push(other);
    }
    let lower = |cp: u32| match LOWER.binary_search_by_key(&cp, |p| p.0) {
        Ok(i) => LOWER[i].1,
        Err(_) => cp,
    };
    let mut cased: Vec<u32> = Vec::new();
    for &(cp, low) in LOWER {
        cased.push(cp);
        cased.push(low);
    }
    for &(k, other) in EXTRA {
        cased.push(k);
        cased.push(other);
    }
    cased.sort_unstable();
    cased.dedup();
    let mut out = v.clone();
    for c in cased {
        if !has(&v, c) {
            continue;
        }
        let k = lower(c);
        let mut keys = vec![k];
        keys.extend(extra.get(&k).cloned().unwrap_or_default());
        for key in keys {
            out.push((key, key));
            for &x in same.get(&key).into_iter().flatten() {
                out.push((x, x));
            }
        }
    }
    norm(out)
}

/// The two-letter general category a long or short property name stands for, or the group of them.
fn category_codes(name: &str) -> Option<Vec<&'static str>> {
    const LONG: &[(&str, &[&str])] = &[
        ("Letter", &["Lu", "Ll", "Lt", "Lm", "Lo"]),
        ("Cased_Letter", &["Lu", "Ll", "Lt"]),
        ("Uppercase_Letter", &["Lu"]),
        ("Lowercase_Letter", &["Ll"]),
        ("Titlecase_Letter", &["Lt"]),
        ("Modifier_Letter", &["Lm"]),
        ("Other_Letter", &["Lo"]),
        ("Mark", &["Mn", "Mc", "Me"]),
        ("Nonspacing_Mark", &["Mn"]),
        ("Spacing_Mark", &["Mc"]),
        ("Enclosing_Mark", &["Me"]),
        ("Number", &["Nd", "Nl", "No"]),
        ("Decimal_Number", &["Nd"]),
        ("Letter_Number", &["Nl"]),
        ("Other_Number", &["No"]),
        ("Punctuation", &["Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po"]),
        ("Connector_Punctuation", &["Pc"]),
        ("Dash_Punctuation", &["Pd"]),
        ("Open_Punctuation", &["Ps"]),
        ("Close_Punctuation", &["Pe"]),
        ("Initial_Punctuation", &["Pi"]),
        ("Final_Punctuation", &["Pf"]),
        ("Other_Punctuation", &["Po"]),
        ("Symbol", &["Sm", "Sc", "Sk", "So"]),
        ("Math_Symbol", &["Sm"]),
        ("Currency_Symbol", &["Sc"]),
        ("Modifier_Symbol", &["Sk"]),
        ("Other_Symbol", &["So"]),
        ("Separator", &["Zs", "Zl", "Zp"]),
        ("Space_Separator", &["Zs"]),
        ("Line_Separator", &["Zl"]),
        ("Paragraph_Separator", &["Zp"]),
        ("Other", &["Cc", "Cf", "Cs", "Co", "Cn"]),
        ("Control", &["Cc"]),
        ("Format", &["Cf"]),
        ("Surrogate", &["Cs"]),
        ("Private_Use", &["Co"]),
        ("Unassigned", &["Cn"]),
    ];
    use crate::regexunicode_tables::CATEGORY_NAMES;
    if let Some((_, codes)) = LONG.iter().find(|(long, _)| *long == name) {
        return Some(codes.to_vec());
    }
    if name == "LC" {
        return Some(vec!["Lu", "Ll", "Lt"]);
    }
    if name == "Any" {
        return Some(CATEGORY_NAMES.to_vec());
    }
    let found: Vec<&'static str> = CATEGORY_NAMES
        .iter()
        .copied()
        .filter(|c| *c == name || (name.len() == 1 && c.starts_with(name)))
        .collect();
    (!found.is_empty()).then_some(found)
}

/// `\p{Name}`: the code points of a general category (`L`, `Lu`, `Letter`, `Uppercase_Letter`, `Nd`, ...), from Python's
/// `unicodedata` for the same Unicode version as `\w`. `None` for a name that is not one.
pub fn property(name: &str) -> Option<Set> {
    use crate::regexunicode_tables::{CATEGORIES, CATEGORY_NAMES};
    let codes = category_codes(name)?;
    let wanted: Vec<usize> = codes
        .iter()
        .filter_map(|c| CATEGORY_NAMES.iter().position(|n| n == c))
        .collect();
    let set: Set = CATEGORIES
        .iter()
        .filter(|&&(_, _, c)| wanted.contains(&(c as usize)))
        .map(|&(lo, hi, _)| (lo, hi))
        .collect();
    Some(without_surrogates(norm(set)))
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

    #[test]
    fn folding_follows_pythons_extra_equivalences() {
        // `k` and the Kelvin sign, `s` and the long s, final and medial sigma, `ǅ` with both other forms.
        assert_eq!(
            fold(&vec![(0x6B, 0x6B)]),
            vec![(0x4B, 0x4B), (0x6B, 0x6B), (0x212A, 0x212A)]
        );
        assert_eq!(
            fold(&vec![(0x3C2, 0x3C2)]),
            vec![(0x3A3, 0x3A3), (0x3C2, 0x3C3)]
        );
        assert_eq!(fold(&vec![(0x1C5, 0x1C5)]), vec![(0x1C4, 0x1C6)]);
    }

    #[test]
    fn properties_name_general_categories() {
        let upper = property("Lu").unwrap();
        assert!(upper.contains(&(0x41, 0x5A)));
        assert_eq!(property("Uppercase_Letter").unwrap(), upper);
        let letters = property("L").unwrap();
        assert!(letters.iter().any(|&(lo, hi)| lo <= 0x4E00 && hi >= 0x4E00));
        assert_eq!(property("Letter").unwrap(), letters);
        assert!(property("Nd").unwrap().contains(&(0x30, 0x39)));
        assert!(property("Greek").is_none());
        // Surrogates are not code points of text.
        assert!(property("Cs").unwrap().is_empty());
    }
}
