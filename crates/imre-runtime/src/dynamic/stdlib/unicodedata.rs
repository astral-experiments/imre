//! Raw Unicode 16 decomposition mappings exposed by CPython 3.14.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::{Context, Error, Id, NONE, Result};

/// The UCD mapping retains compatibility tags and only one level of decomposition.
static MAPPINGS: LazyLock<HashMap<u32, &'static str>> = LazyLock::new(|| {
    include_str!("../../../native/ucd16-decomposition.txt")
        .lines()
        .map(|line| {
            let (point, mapping) = line.split_once(' ').expect("Unicode decomposition record");
            (
                u32::from_str_radix(point, 16).expect("Unicode code point"),
                mapping,
            )
        })
        .collect()
});

/// Return the UCD mapping for one Python character, including lone surrogates.
pub(super) fn decomposition(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    if has_keywords {
        return Err(Error::new(
            "TypeError",
            "unicodedata.decomposition() takes no keyword arguments",
        ));
    }
    let [value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "unicodedata.decomposition() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let points = ctx.string_codepoints(*value).map_err(|_| {
        Error::new(
            "TypeError",
            format!(
                "decomposition() argument must be a unicode character, not {}",
                if *value == NONE {
                    "None"
                } else {
                    ctx.type_name(*value)
                }
            ),
        )
    })?;
    let [point] = points.as_ref() else {
        return Err(Error::new(
            "TypeError",
            format!(
                "decomposition(): argument must be a unicode character, not a string of length {}",
                points.len()
            ),
        ));
    };
    let point = *point;
    if (0xAC00..=0xD7A3).contains(&point) {
        // Section 3.12.2 of the Unicode Standard decomposes Hangul syllables algorithmically into Jamo.
        let index = point - 0xAC00;
        let leading = 0x1100 + index / (21 * 28);
        let vowel = 0x1161 + index / 28 % 21;
        let trailing = index % 28;
        return Ok(ctx.text(if trailing == 0 {
            format!("{leading:04X} {vowel:04X}")
        } else {
            format!("{leading:04X} {vowel:04X} {:04X}", 0x11A7 + trailing)
        }));
    }
    Ok(ctx.text(MAPPINGS.get(&point).copied().unwrap_or("")))
}
