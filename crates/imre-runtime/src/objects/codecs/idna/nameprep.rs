//! RFC 3491 Nameprep using the Unicode versions selected by CPython 3.14.
use std::sync::LazyLock;

use crate::objects::{Form, legacy};

use super::LabelError;
use crate::integer_text::newer_printable;
use crate::objects::exceptions::string_repr;

struct Tables {
    // Sorted inclusive ranges prohibited by RFC 3491.
    prohibited: Vec<(u32, u32)>,
    // Only overrides to the ordinary single-character lowercase mapping.
    mappings: Vec<(char, String)>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let mut prohibited = Vec::new();
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
            prohibited.push((start, points.next().unwrap()));
        }
    }
    Tables {
        prohibited,
        mappings,
    }
});

fn prohibited(point: u32) -> bool {
    let i = TABLES.prohibited.partition_point(|&(_, end)| end < point);
    TABLES
        .prohibited
        .get(i)
        .is_some_and(|&(start, _)| start <= point)
}

fn map(point: u32, output: &mut Vec<u32>) {
    let Some(ch) = char::from_u32(point) else {
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
        let lower = unicode_case_mapping::to_lowercase(ch);
        if lower[0] == 0 {
            output.push(point);
        } else {
            output.extend(lower.into_iter().take_while(|&n| n != 0));
        }
    }
}

/// Apply Nameprep to a single label, reporting character offsets in its prepared form.
pub(super) fn prepare(label: &[u32]) -> Result<String, LabelError> {
    let mut mapped = Vec::with_capacity(label.len());
    for &point in label {
        map(point, &mut mapped);
    }
    let chars = legacy::normalize(Form::Nfkc, &mapped);
    let mut has_randal = false;
    let mut first_l = None;
    for (i, &ch) in chars.iter().enumerate() {
        if prohibited(ch) {
            let value = match char::from_u32(ch) {
                Some(c) => string_repr(c.encode_utf8(&mut [0; 4])).to_string(),
                None => format!("'\\u{ch:04x}'"),
            };
            return Err(
                LabelError::new(i, i + 1, format!("Invalid character {value}")).with_text(chars),
            );
        }
        match legacy::bidirectional(ch) {
            "R" | "AL" => has_randal = true,
            "L" if first_l.is_none() => first_l = Some(i),
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
        if !matches!(legacy::bidirectional(chars[0]), "R" | "AL") {
            return Err(LabelError::new(0, 1, "Violation of BIDI requirement 3").with_text(chars));
        }
        if !matches!(legacy::bidirectional(chars[last]), "R" | "AL") {
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
