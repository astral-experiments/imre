//! Apply a built-in reached through a multibyte encoder's named error handler.
use super::super::wide_unicode;
use super::{Codec, Context, Id, NONE, Result, Value, type_error};

pub(super) fn replacement(
    ctx: &mut Context,
    name: &str,
    exc: Id,
    original: Id,
    source: &[u32],
    pos: usize,
    end: usize,
) -> Result<Id> {
    if name == "strict" {
        return ctx.op("raise", exc, NONE, NONE);
    }
    let mut wide = None;
    if name == "surrogatepass" {
        let encoding = ctx.lookup_attr(exc, "encoding")?;
        if ctx.string_storage(encoding).is_none() {
            return Err(type_error(
                "UnicodeError 'encoding' attribute must be a string",
            ));
        }
        let spelling = ctx.encode_utf8_codepoints(encoding, false, "strict")?;
        let spelling = std::str::from_utf8(&spelling).expect("strict UTF-8");
        let spelling = spelling.split('\0').next().unwrap_or_default();
        wide = wide_unicode::surrogate_encoding(spelling);
        if wide.is_none()
            && spelling != "cp65001"
            && !["utf-8", "utf_8", "utf8"]
                .iter()
                .any(|s| spelling.eq_ignore_ascii_case(s))
        {
            return ctx.op("raise", exc, NONE, NONE);
        }
    }

    // CPython's error handlers clamp start/end against the possibly rebound exc.object.
    // A Shift JIS encoding error covers one character, even for adjacent failures.
    debug_assert_eq!(end, pos + 1);
    let object = ctx.lookup_attr(exc, "object")?;
    let changed;
    let points = if object == original {
        source
    } else {
        if ctx.string_storage(object).is_none() {
            return Err(type_error(
                "UnicodeError 'object' attribute must be a string",
            ));
        }
        changed = ctx.string_codepoints(object)?;
        &changed
    };
    let end = end.min(points.len());
    let point = points.get(pos.min(points.len().saturating_sub(1))).copied();
    let value = match (name, point) {
        ("surrogatepass" | "surrogateescape", None) => ctx.alloc(Value::Bytes(Vec::new())),
        ("surrogateescape", Some(p @ 0xdc80..=0xdcff)) => ctx.alloc(Value::Bytes(vec![p as u8])),
        ("surrogatepass", Some(p @ 0xd800..=0xdfff)) => {
            let mut bytes = Vec::new();
            if let Some(format) = wide {
                format.append(&mut bytes, [p])?;
            } else {
                let text = ctx.text_codepoints(vec![p]);
                bytes = ctx.encode_utf8_codepoints(text, false, "surrogatepass")?;
            }
            ctx.alloc(Value::Bytes(bytes))
        }
        ("surrogateescape" | "surrogatepass", _) => return ctx.op("raise", exc, NONE, NONE),
        ("ignore", _) | (_, None) => ctx.text(""),
        (name, Some(p)) => ctx.text(Codec::error_replacement(name, p)),
    };
    let resume = ctx.alloc(Value::Int(crate::Int::from_usize(end)));
    Ok(ctx.alloc(Value::Tuple(vec![value, resume])))
}
