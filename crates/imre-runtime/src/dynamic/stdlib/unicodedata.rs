//! Current and historical Unicode character properties exposed by CPython 3.14.

use crate::dynamic::value::SlotStorage;
use std::collections::HashMap;
use std::sync::LazyLock;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{Form, legacy, normalize_unicode, ucd_character};

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
fn character(
    ctx: &Context,
    owner: &str,
    name: &str,
    args: &[Id],
    has_keywords: bool,
) -> Result<u32> {
    if has_keywords {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    let [value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{owner}.{name}() takes exactly one argument ({} given)",
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
    let point = character(ctx, "unicodedata", "east_asian_width", args, has_keywords)?;
    Ok(ctx.text(crate::objects::east_asian_width(point)))
}

pub(super) fn bidirectional(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    let point = character(ctx, "unicodedata", "bidirectional", args, has_keywords)?;
    Ok(ctx.text(crate::objects::bidirectional(point)))
}

pub(super) fn category(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    let point = character(ctx, "unicodedata", "category", args, has_keywords)?;
    Ok(ctx.text(crate::objects::general_category(point)))
}

pub(super) fn decomposition(ctx: &mut Context, args: &[Id], has_keywords: bool) -> Result<Id> {
    let point = character(ctx, "unicodedata", "decomposition", args, has_keywords)?;
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

const CATEGORY_DOC: &str = "Returns the general category assigned to the character chr as string.";
const CHARACTER_SIGNATURE: &str = "($self, chr, /)";
pub(in crate::dynamic) const CLASS: &str = "unicodedata.UCD";

/// Install the database type and its historical instance for this module generation.
pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("unicodedata", Some(scope));
    let module_name = ctx.text("unicodedata");
    ctx.bind(fields, "__module__", module_name);
    let name = ctx.text("UCD");
    ctx.bind(fields, "__qualname__", name);
    let object = ctx.builtin("object");
    let owner = ctx.allocate_class("UCD".into(), fields, vec![object])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *heap_type = Some(CLASS);
    let Value::Scope(frame) = ctx.heap.get_mut(fields) else {
        unreachable!()
    };
    let namespace = frame.class_namespace.as_mut().unwrap();
    namespace.slot_names = Some([].into());
    namespace.immutable_type = true;
    namespace.defining_module = Some(module);
    for name in ["category", "bidirectional", "normalize", "is_normalized"] {
        let method = ctx.alloc(Value::HeapTypeMethod {
            owner,
            name,
            slot_wrapper: false,
        });
        ctx.bind(fields, name, method);
    }
    let name = ctx.text("unidata_version");
    let version = ctx.text("3.2.0");
    let descriptor = ctx.alloc(Value::SlotMember {
        owner,
        name,
        storage: SlotStorage::ReadOnlyText(version),
    });
    ctx.bind(fields, "unidata_version", descriptor);
    let legacy = ctx.instance(owner, None);
    ctx.bind(scope, "UCD", owner);
    ctx.bind(scope, "ucd_3_2_0", legacy);
    Ok(())
}

pub(in crate::dynamic) fn method_metadata(name: &str, attribute: &str) -> Option<&'static str> {
    match attribute {
        "__doc__" => Some(match name {
            "category" => CATEGORY_DOC,
            "bidirectional" => {
                "Returns the bidirectional class assigned to the character chr as string.\n\nIf no such value is defined, an empty string is returned."
            }
            "normalize" => {
                "Return the normal form 'form' for the Unicode string unistr.\n\nValid values for form are 'NFC', 'NFKC', 'NFD', and 'NFKD'."
            }
            "is_normalized" => {
                "Return whether the Unicode string unistr is in the normal form 'form'.\n\nValid values for form are 'NFC', 'NFKC', 'NFD', and 'NFKD'."
            }
            _ => return None,
        }),
        "__text_signature__" => Some(match name {
            "category" | "bidirectional" => CHARACTER_SIGNATURE,
            "normalize" | "is_normalized" => "($self, form, unistr, /)",
            _ => return None,
        }),
        _ => None,
    }
}

pub(in crate::dynamic) fn legacy_method(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method UCD.{name}() needs an argument"),
        ));
    };
    ctx.check_heap_method_receiver(owner, name, receiver)?;
    if matches!(name, "normalize" | "is_normalized") {
        return normalization(ctx, "UCD", name, rest, !kwargs.is_empty());
    }
    let point = character(ctx, "UCD", name, rest, !kwargs.is_empty())?;
    let value = match name {
        "category" => legacy::category(point),
        "bidirectional" => legacy::bidirectional(point),
        _ => unreachable!(),
    };
    Ok(ctx.text(value))
}

pub(super) fn normalization(
    ctx: &mut Context,
    owner: &str,
    name: &str,
    args: &[Id],
    has_keywords: bool,
) -> Result<Id> {
    if has_keywords {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    let [form, text] = args else {
        return Err(Error::new(
            "TypeError",
            format!("{name} expected 2 arguments, got {}", args.len()),
        ));
    };
    for (index, &value) in args.iter().enumerate() {
        if !matches!(
            ctx.heap.get(ctx.scalar_value(value)),
            Value::Str(_) | Value::SurrogateStr(_)
        ) {
            let kind = if value == NONE {
                "None"
            } else {
                ctx.type_name(value)
            };
            return Err(Error::new(
                "TypeError",
                format!("{name}() argument {} must be str, not {kind}", index + 1),
            ));
        }
    }
    let text = ctx.scalar_value(*text);
    let check = name == "is_normalized";
    // A form containing a surrogate cannot name a normal form. The empty
    // input shortcut still applies before validation, as in CPython.
    let form = ctx.str_value(*form).unwrap_or("");
    if owner == "unicodedata"
        && let Value::Str(input) = ctx.heap.get(text)
    {
        let result = normalize_unicode(form, input)?;
        return if check {
            Ok(super::builtins::boolean(
                result.as_ref().is_none_or(|s| s == input),
            ))
        } else {
            Ok(result.map_or_else(|| text, |s| ctx.text(s)))
        };
    }
    if let Value::Str(input) = ctx.heap.get(text)
        && input.is_ascii()
    {
        if input.is_empty() {
            return Ok(if check { super::TRUE } else { text });
        }
        Form::parse(form)?;
        // The historical database creates a base string even when its contents are normalized.
        return Ok(if check {
            super::TRUE
        } else {
            let result = input.clone();
            ctx.text(result)
        });
    }
    let points = ctx.string_codepoints(text)?;
    if points.is_empty() {
        return Ok(if check { super::TRUE } else { text });
    }
    let parsed = Form::parse(form)?;
    if owner == "UCD" {
        // Historical quick checks are disabled; return a fresh base string.
        let normalized = legacy::normalize(parsed, &points);
        return Ok(if check {
            super::builtins::boolean(normalized == *points)
        } else {
            ctx.text_codepoints(normalized)
        });
    }
    // Current normalization treats lone surrogates as inert starters.
    let mut result = Vec::with_capacity(points.len());
    let mut can_reuse = true;
    for run in points.split_inclusive(|p| (0xd800..=0xdfff).contains(p)) {
        let (scalars, surrogate) = match run.split_last() {
            Some((&last, prefix)) if (0xd800..=0xdfff).contains(&last) => (prefix, Some(last)),
            _ => (run, None),
        };
        let segment: String = scalars
            .iter()
            .map(|&p| char::from_u32(p).unwrap())
            .collect();
        if let Some(normalized) = normalize_unicode(form, &segment)? {
            result.extend(normalized.chars().map(u32::from));
            can_reuse = false;
        } else {
            result.extend_from_slice(scalars);
        }
        result.extend(surrogate);
    }
    Ok(if check {
        super::builtins::boolean(result == *points)
    } else if can_reuse {
        text
    } else {
        ctx.text_codepoints(result)
    })
}
