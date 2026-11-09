//! CPython 3.14's UCD 3.2.0, also used by its RFC 3491 Nameprep profile.

use crate::integer_text::newer_printable;
use unicode_normalization::char::{
    canonical_combining_class, compose, decompose_canonical, decompose_compatible,
};

include!("../../../native/ucd3.2.rs");

fn property(point: u32, boundaries: &[u32], values: &[&'static str]) -> &'static str {
    let end = boundaries.partition_point(|&boundary| boundary >> 5 <= point);
    values[(boundaries[end - 1] & 31) as usize]
}

pub(crate) fn category(point: u32) -> &'static str {
    match property(point, CATEGORY, CATEGORY_VALUES) {
        "" => super::general_category(point),
        changed => changed,
    }
}

pub(crate) fn bidirectional(point: u32) -> &'static str {
    property(point, BIDI, BIDI_VALUES)
}

// Modules/unicodedata.c uses historical decomposition but *current* canonical
// ordering and composition, even for characters unassigned in UCD 3.2.
fn combining(point: u32) -> u8 {
    char::from_u32(point)
        .filter(|&ch| !newer_printable(ch))
        .map_or(0, canonical_combining_class)
}

fn composite(a: u32, b: u32) -> Option<u32> {
    compose(char::from_u32(a)?, char::from_u32(b)?)
        .filter(|&ch| !newer_printable(ch))
        .map(u32::from)
}

fn decompose(point: u32, compatibility: bool, output: &mut Vec<u32>) {
    // NormalizationCorrections.txt, corrections made in Unicode 4.0.0.
    let ch = match point {
        0x2f868 => '\u{2136a}',
        0x2f874 => '\u{5f33}',
        0x2f91f => '\u{43ab}',
        0x2f95f => '\u{7aae}',
        0x2f9bf => '\u{4d57}',
        _ => match char::from_u32(point) {
            Some(ch) if category(point) != "Cn" => ch,
            _ => {
                output.push(point);
                return;
            }
        },
    };
    let emit = |c| output.push(u32::from(c));
    if compatibility {
        decompose_compatible(ch, emit);
    } else {
        decompose_canonical(ch, emit);
    }
}

/// Normalize arbitrary Python code points, including lone surrogates.
pub(crate) fn normalize(form: super::normalization::Form, points: &[u32]) -> Vec<u32> {
    use super::normalization::Form;
    let compatibility = matches!(form, Form::Nfkc | Form::Nfkd);
    let mut chars = Vec::with_capacity(points.len());
    for &point in points {
        decompose(point, compatibility, &mut chars);
    }
    // Stable ordering matters for successive combining marks of equal class.
    for marks in chars.split_mut(|&c| combining(c) == 0) {
        marks.sort_by_key(|&c| combining(c));
    }
    if matches!(form, Form::Nfd | Form::Nfkd) {
        return chars;
    }

    let mut output = Vec::with_capacity(chars.len());
    let mut starter: Option<usize> = None;
    let mut last_class = 0;
    for ch in chars {
        let ccc = combining(ch);
        if let Some(i) = starter
            && (last_class == 0 || last_class < ccc)
            && let Some(composed) = composite(output[i], ch)
        {
            output[i] = composed;
            continue;
        }
        if ccc == 0 {
            starter = Some(output.len());
        }
        output.push(ch);
        last_class = ccc;
    }
    output
}
