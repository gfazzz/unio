//! Canonical normalization (NFD, NFC) and extended grapheme clusters for `std.text` (S-18,
//! docs/language-contracts.md section 4), on the tables of one pinned version of the Unicode
//! Character Database ([`tables::UNICODE_VERSION`]). The algorithms are those of the Unicode
//! Standard, section 3.11 (canonical ordering and composition) and 3.12 (Hangul), and of
//! UAX #29 (grapheme cluster boundaries) of that version.

use super::unicode_tables as tables;

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// The Canonical_Combining_Class of a character.
pub fn combining_class(c: char) -> u8 {
    let cp = c as u32;
    match tables::COMBINING_CLASS.binary_search_by(|&(first, last, _)| {
        if last < cp {
            std::cmp::Ordering::Less
        } else if first > cp {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => tables::COMBINING_CLASS[i].2,
        Err(_) => 0,
    }
}

fn decompose_into(c: char, out: &mut Vec<char>) {
    let cp = c as u32;
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let index = cp - S_BASE;
        let parts = [
            L_BASE + index / N_COUNT,
            V_BASE + (index % N_COUNT) / T_COUNT,
            T_BASE + index % T_COUNT,
        ];
        let count = if index.is_multiple_of(T_COUNT) { 2 } else { 3 };
        out.extend(parts[..count].iter().filter_map(|&p| char::from_u32(p)));
        return;
    }
    match tables::DECOMPOSITION.binary_search_by_key(&cp, |&(c, _, _)| c) {
        Ok(i) => {
            let (_, first, second) = tables::DECOMPOSITION[i];
            for part in [first, second] {
                if let Some(p) = char::from_u32(part).filter(|_| part != 0) {
                    decompose_into(p, out);
                }
            }
        }
        Err(_) => out.push(c),
    }
}

/// The canonical decomposition with canonical ordering: a stable sort of every run of
/// nonzero combining classes by class.
fn decompose(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        decompose_into(c, &mut out);
    }
    let mut start = 0;
    while start < out.len() {
        if combining_class(out[start]) == 0 {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < out.len() && combining_class(out[end]) != 0 {
            end += 1;
        }
        out[start..end].sort_by_key(|&c| combining_class(c));
        start = end;
    }
    out
}

fn compose_pair(a: char, b: char) -> Option<char> {
    let (a, b) = (a as u32, b as u32);
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return char::from_u32(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return char::from_u32(a + (b - T_BASE));
    }
    tables::COMPOSITION
        .binary_search_by_key(&(a, b), |&(x, y, _)| (x, y))
        .ok()
        .and_then(|i| char::from_u32(tables::COMPOSITION[i].2))
}

/// Normalization Form D.
pub fn nfd(text: &str) -> String {
    decompose(text).into_iter().collect()
}

/// Normalization Form C: the canonical decomposition, then the canonical composition, where
/// a character combines with the last starter unless a character between them blocks it.
pub fn nfc(text: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut starter: Option<usize> = None;
    let mut last_class: Option<u8> = None;
    for c in decompose(text) {
        let class = combining_class(c);
        if let Some(s) = starter {
            if last_class.is_none_or(|last| last < class) {
                if let Some(combined) = compose_pair(out[s], c) {
                    out[s] = combined;
                    continue;
                }
            }
        }
        if class == 0 {
            starter = Some(out.len());
            last_class = None;
        } else {
            last_class = Some(class);
        }
        out.push(c);
    }
    out.into_iter().collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Other,
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
}

const CLASSES: [Class; 14] = [
    Class::Other,
    Class::Cr,
    Class::Lf,
    Class::Control,
    Class::Extend,
    Class::Zwj,
    Class::RegionalIndicator,
    Class::Prepend,
    Class::SpacingMark,
    Class::L,
    Class::V,
    Class::T,
    Class::Lv,
    Class::Lvt,
];

fn in_ranges<T>(ranges: &[T], cp: u32, bounds: impl Fn(&T) -> (u32, u32)) -> Option<usize> {
    ranges
        .binary_search_by(|r| {
            let (first, last) = bounds(r);
            if last < cp {
                std::cmp::Ordering::Less
            } else if first > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .ok()
}

fn class_of(c: char) -> Class {
    match in_ranges(tables::GRAPHEME_CLASS, c as u32, |r| (r.0, r.1)) {
        Some(i) => CLASSES[tables::GRAPHEME_CLASS[i].2 as usize],
        None => Class::Other,
    }
}

fn is_pictographic(c: char) -> bool {
    in_ranges(tables::EXTENDED_PICTOGRAPHIC, c as u32, |r| (r.0, r.1)).is_some()
}

/// The extended grapheme clusters of the text, by the rules GB3 to GB999 of UAX #29.
pub fn graphemes(text: &str) -> Vec<&str> {
    let mut clusters = Vec::new();
    let mut chars = text.char_indices();
    let Some((_, first)) = chars.next() else {
        return clusters;
    };
    let mut start = 0;
    let mut previous = class_of(first);
    // GB11: an Extended_Pictographic followed by Extend characters, then a ZWJ.
    let mut pictographic = is_pictographic(first);
    let mut after_zwj_of_pictographic = false;
    // GB12, GB13: the number of regional indicators in the run that ends with `previous`.
    let mut regional = usize::from(previous == Class::RegionalIndicator);
    for (at, c) in chars {
        let class = class_of(c);
        let picto = is_pictographic(c);
        let join = match (previous, class) {
            (Class::Cr, Class::Lf) => true,
            (Class::Cr | Class::Lf | Class::Control, _) => false,
            (_, Class::Cr | Class::Lf | Class::Control) => false,
            (Class::L, Class::L | Class::V | Class::Lv | Class::Lvt) => true,
            (Class::Lv | Class::V, Class::V | Class::T) => true,
            (Class::Lvt | Class::T, Class::T) => true,
            (_, Class::Extend | Class::Zwj | Class::SpacingMark) => true,
            (Class::Prepend, _) => true,
            (Class::Zwj, _) if after_zwj_of_pictographic && picto => true,
            (Class::RegionalIndicator, Class::RegionalIndicator) => regional % 2 == 1,
            _ => false,
        };
        if !join {
            clusters.push(&text[start..at]);
            start = at;
        }
        // The state of GB11 after this character.
        after_zwj_of_pictographic = class == Class::Zwj && pictographic;
        pictographic = if picto {
            true
        } else {
            pictographic && class == Class::Extend
        };
        regional = if class == Class::RegionalIndicator {
            if join {
                regional + 1
            } else {
                1
            }
        } else {
            0
        };
        previous = class;
    }
    clusters.push(&text[start..]);
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_composes_and_decomposes_accents_and_hangul() {
        assert_eq!(nfd("\u{E9}"), "e\u{301}");
        assert_eq!(nfc("e\u{301}"), "\u{E9}");
        // Canonical ordering: dot below (220) before acute (230).
        assert_eq!(nfd("q\u{301}\u{323}"), "q\u{323}\u{301}");
        assert_eq!(nfc("\u{1E0B}\u{323}"), "\u{1E0D}\u{307}");
        assert_eq!(nfd("\u{D4DB}"), "\u{1111}\u{1171}\u{11B6}");
        assert_eq!(nfc("\u{1111}\u{1171}\u{11B6}"), "\u{D4DB}");
        // A composition exclusion stays decomposed; a singleton is replaced.
        assert_eq!(nfc("\u{958}"), "\u{915}\u{93C}");
        assert_eq!(nfc("\u{212B}"), "\u{C5}");
        // A blocked mark does not combine.
        assert_eq!(
            nfc("a\u{315}\u{300}\u{5AE}\u{300}b"),
            "\u{E0}\u{5AE}\u{300}\u{315}b"
        );
        assert_eq!(nfc(""), "");
    }

    #[test]
    fn grapheme_clusters_follow_uax_29() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        assert_eq!(graphemes(family), vec![family]);
        assert_eq!(graphemes("e\u{301}x"), vec!["e\u{301}", "x"]);
        assert_eq!(graphemes("\r\n\n"), vec!["\r\n", "\n"]);
        let flags = "\u{1F1FA}\u{1F1F8}\u{1F1EB}\u{1F1F7}\u{1F1E9}";
        assert_eq!(
            graphemes(flags),
            vec!["\u{1F1FA}\u{1F1F8}", "\u{1F1EB}\u{1F1F7}", "\u{1F1E9}"]
        );
        assert_eq!(
            graphemes("\u{1100}\u{1161}\u{11A8}a"),
            vec!["\u{1100}\u{1161}\u{11A8}", "a"]
        );
        assert_eq!(
            graphemes("a\u{200D}\u{1F469}"),
            vec!["a\u{200D}", "\u{1F469}"]
        );
        assert_eq!(graphemes("\u{1F44D}\u{1F3FD}"), vec!["\u{1F44D}\u{1F3FD}"]);
        assert_eq!(graphemes("\u{600}1"), vec!["\u{600}1"]);
        assert!(graphemes("").is_empty());
    }
}
