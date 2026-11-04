//! RFC 3491 Nameprep using the Unicode versions selected by CPython 3.14.
use std::sync::LazyLock;

use unicode_normalization::char::{canonical_combining_class, compose, decompose_compatible};

use super::LabelError;
use crate::integer_text::newer_printable;
use crate::objects::exceptions::string_repr;

struct Tables {
    // Sorted start, end (inclusive), and RFC 3454 class P (prohibited), R (D.1), or L (D.2).
    classes: Vec<(u32, u32, u8)>,
    // Only overrides to the ordinary single-character lowercase + NFKD mapping.
    mappings: Vec<(char, String)>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let mut classes = Vec::new();
    let mut mappings = Vec::new();
    for row in include_str!("../../../../native/idna-nameprep.txt").lines() {
        let mut fields = row.split_ascii_whitespace();
        let kind = fields.next().unwrap();
        let mut points = fields.map(|s| u32::from_str_radix(s, 16).unwrap());
        let start = points.next().unwrap();
        if kind == "M" {
            let mapped = points.map(|c| char::from_u32(c).unwrap()).collect();
            mappings.push((char::from_u32(start).unwrap(), mapped));
        } else {
            classes.push((start, points.next().unwrap(), kind.as_bytes()[0]));
        }
    }
    Tables { classes, mappings }
});

fn class(point: u32) -> u8 {
    let i = TABLES.classes.partition_point(|&(_, end, _)| end < point);
    match TABLES.classes.get(i) {
        Some(&(start, _, kind)) if start <= point => kind,
        _ => 0,
    }
}

// CPython's old UCD uses the old decomposition mappings followed by the
// current UCD's canonical ordering and composition (Modules/unicodedata.c).
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

fn decompose(point: u32, output: &mut Vec<u32>) {
    let Some(ch) = char::from_u32(point) else {
        // Surrogates have no decomposition and act as starters. Like CPython,
        // report the prohibition only after normalizing the entire label.
        output.push(point);
        return;
    };
    if let Ok(i) = TABLES
        .mappings
        .binary_search_by_key(&ch, |&(point, _)| point)
    {
        output.extend(TABLES.mappings[i].1.chars().map(u32::from));
    } else if newer_printable(ch) {
        output.push(point);
    } else {
        let mut emit = |c| output.push(u32::from(c));
        let lower = unicode_case_mapping::to_lowercase(ch);
        if lower[0] == 0 {
            decompose_compatible(ch, emit);
        } else {
            for point in lower.into_iter().take_while(|&n| n != 0) {
                decompose_compatible(char::from_u32(point).unwrap(), &mut emit);
            }
        }
    }
}

fn normalized(label: &[u32]) -> Vec<u32> {
    let mut chars = Vec::with_capacity(label.len());
    for &point in label {
        decompose(point, &mut chars);
    }

    // Keep equal-class combining marks in their original relative order.
    for marks in chars.split_mut(|&c| combining(c) == 0) {
        marks.sort_by_key(|&c| combining(c));
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

/// Apply Nameprep to a single label, reporting character offsets in its prepared form.
pub(super) fn prepare(label: &[u32]) -> Result<String, LabelError> {
    let chars = normalized(label);
    let mut has_randal = false;
    let mut first_l = None;
    for (i, &ch) in chars.iter().enumerate() {
        match class(ch) {
            b'P' => {
                let value = match char::from_u32(ch) {
                    Some(c) => string_repr(c.encode_utf8(&mut [0; 4])).to_string(),
                    None => format!("'\\u{ch:04x}'"),
                };
                return Err(
                    LabelError::new(i, i + 1, format!("Invalid character {value}"))
                        .with_text(chars),
                );
            }
            b'R' => has_randal = true,
            b'L' if first_l.is_none() => first_l = Some(i),
            _ => {}
        }
    }
    if has_randal {
        if let Some(i) = first_l {
            return Err(
                LabelError::new(i, i + 1, "Violation of BIDI requirement 2").with_text(chars)
            );
        }
        let last = chars.len() - 1;
        if class(chars[0]) != b'R' {
            return Err(LabelError::new(0, 1, "Violation of BIDI requirement 3").with_text(chars));
        }
        if class(chars[last]) != b'R' {
            return Err(
                LabelError::new(last, last + 1, "Violation of BIDI requirement 3").with_text(chars),
            );
        }
    }
    Ok(chars
        .into_iter()
        .map(|c| char::from_u32(c).expect("Nameprep prohibits surrogates"))
        .collect())
}
