//! Unicode 16 character properties exposed by CPython 3.14.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::ucd_character;

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

/// Parse an Argument Clinic positional-only Unicode character, including lone surrogates.
fn character(ctx: &Context, name: &str, args: &[Id], has_keywords: bool) -> Result<u32> {
    if has_keywords {
        return Err(Error::new(
            "TypeError",
            format!("unicodedata.{name}() takes no keyword arguments"),
        ));
    }
    let [value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "unicodedata.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    match ctx.heap.get(ctx.scalar_value(*value)) {
        Value::Str(text) => Ok(ucd_character(name, text.chars().map(u32::from))?),
        Value::SurrogateStr(points) => Ok(ucd_character(name, points.iter().copied())?),
        _ => {
            let actual = if *value == NONE {
                "None".into()
            } else {
                super::diagnostic_type_name(ctx, *value)
            };
            // _PyArg_BadArgument limits the UTF-8 type name to 50 bytes.
            let actual = &actual[..actual.floor_char_boundary(50)];
            Err(Error::new(
                "TypeError",
                format!("{name}() argument must be a unicode character, not {actual}"),
            ))
        }
    }
}

pub(super) fn east_asian_width(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    let point = character(ctx, "east_asian_width", args, has_keywords)?;
    Ok(ctx.text(crate::objects::east_asian_width(point)))
}

pub(super) fn decomposition(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    let point = character(ctx, "decomposition", args, has_keywords)?;
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
