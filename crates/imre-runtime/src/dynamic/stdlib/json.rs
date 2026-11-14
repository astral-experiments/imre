mod encoder_state;
mod notes;
mod scanstring;
pub(super) use encoder_state::encoder_state;
use notes::{Location, noted};
pub(super) use scanstring::scanstring;

use std::collections::HashMap;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, required_args};
use crate::objects::{
    DecodedJson, JsonNumber, JsonTokenError, decode_json_tokens_at, write_json_codepoints,
    write_json_fragment, write_json_string,
};
use crate::{Str, StrOwner};

/// Recognize an exact dict's bound getter without inspecting an arbitrary parse_constant hook.
pub(super) fn is_constants_lookup(ctx: &Context, constants: Id, hook: Id) -> bool {
    let Value::Method {
        receiver,
        callable,
        explicit: false,
    } = ctx.heap.get(hook)
    else {
        return false;
    };
    *receiver == constants
        && matches!(ctx.heap.get(constants), Value::Dict(_))
        && matches!(ctx.heap.get(*callable), Value::Native(name) if name == "dict.__getitem__")
}

/// Scan one value at a Python character offset and return its value and end offset.
pub(super) fn scan_once(
    ctx: &mut Context,
    constant: Id,
    input: Id,
    idx: Id,
    pairs_hook: Id,
) -> Result<Id> {
    let index = ctx.index_integer(idx)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })?;
    let start =
        usize::try_from(index).map_err(|_| Error::new("ValueError", "idx cannot be negative"))?;
    let (text, wide) = match ctx.heap.get(input) {
        Value::Str(text) => (
            Str::try_new(text).map_err(|_| crate::Error::allocation())?,
            None,
        ),
        Value::SurrogateStr(points) => {
            // JSON syntax is ASCII. One byte per Python character preserves offsets,
            // while the string builder reads token contents from the original input.
            let mut syntax = String::new();
            syntax
                .try_reserve_exact(points.len())
                .map_err(|_| crate::Error::allocation())?;
            syntax.extend(
                points
                    .iter()
                    .map(|&p| if p < 128 { char::from(p as u8) } else { '?' }),
            );
            (Str::from(syntax), Some(input))
        }
        _ => return Err(Error::new("TypeError", "scanner input must be a string")),
    };
    let Some(byte) = text.char_indices().nth(start).map(|(offset, _)| offset) else {
        return Err(ctx.exception("StopIteration", &[idx]));
    };
    match parse(ctx, &text, wide, byte, constant, pairs_hook) {
        Ok((value, end)) => {
            let end = ctx.int(index + text[byte..end].chars().count() as i64);
            Ok(ctx.alloc(Value::Tuple(vec![value, end])))
        }
        Err(JsonTokenError::Build(error)) => Err(error),
        Err(JsonTokenError::Parse(error)) => {
            let Some((message, _, pos)) = error.json_decode_fields() else {
                return Err(error.into());
            };
            if message == "Expecting value" {
                let pos = ctx.int(pos as i64);
                Err(ctx.exception("StopIteration", &[pos]))
            } else {
                Err(scanstring::scan_error(ctx, input, pos as i64, message))
            }
        }
    }
}

/// The key memo compares decoded contents, including literal and escaped spellings of a key.
#[derive(Eq, PartialEq, Hash)]
enum ParsedString {
    Text(String),
    Surrogates(Vec<u32>),
}

impl ParsedString {
    fn read(
        ctx: &Context,
        syntax: &str,
        wide: Option<Id>,
        body: std::ops::Range<usize>,
    ) -> std::result::Result<Self, crate::Error> {
        let points = if let Some(id) = wide {
            let Value::SurrogateStr(points) = ctx.heap.get(id) else {
                unreachable!()
            };
            let points = &points[body];
            scanstring::decoded_points(points.iter().copied(), points.len())?
        } else {
            // Keep the scalar parser's fast path. The syntax cursor has already validated this
            // token; serde rejects it only if its decoded contents are outside Rust's str domain.
            let token = &syntax[body.start - 1..body.end + 1];
            if let Ok(value) = serde_json::from_str::<String>(token) {
                return Ok(Self::Text(value));
            }
            let text = &syntax[body];
            scanstring::decoded_points(text.chars().map(u32::from), text.len())?
        };
        if points.iter().any(|p| (0xd800..=0xdfff).contains(p)) {
            return Ok(Self::Surrogates(points));
        }
        let chars = points.iter().map(|p| char::from_u32(*p).unwrap());
        let size = chars.clone().map(char::len_utf8).sum();
        let mut text = String::new();
        text.try_reserve_exact(size)
            .map_err(|_| crate::Error::allocation())?;
        text.extend(chars);
        Ok(Self::Text(text))
    }

    fn boxed(self, ctx: &mut Context) -> Id {
        match self {
            Self::Text(s) => ctx.text(s),
            Self::Surrogates(points) => ctx.alloc(Value::SurrogateStr(points)),
        }
    }

    fn boxed_key(&self, ctx: &mut Context) -> std::result::Result<Id, crate::Error> {
        match self {
            Self::Text(s) => {
                let mut copy = String::new();
                copy.try_reserve_exact(s.len())
                    .map_err(|_| crate::Error::allocation())?;
                copy.push_str(s);
                Ok(ctx.text(copy))
            }
            Self::Surrogates(points) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(points.len())
                    .map_err(|_| crate::Error::allocation())?;
                copy.extend_from_slice(points);
                Ok(ctx.alloc(Value::SurrogateStr(copy)))
            }
        }
    }
}

fn parse(
    ctx: &mut Context,
    text: &str,
    wide: Option<Id>,
    start: usize,
    constant: Id,
    pairs_hook: Id,
) -> std::result::Result<(Id, usize), JsonTokenError<Error>> {
    let mut keys = HashMap::new();
    // A callback can collect while the scanner's per-document key memo is live.
    let key_roots = (pairs_hook != NONE).then(|| ctx.alloc(Value::List(Vec::new())));
    decode_json_tokens_at(
        text,
        start,
        || Str::try_new(text).ok().map(StrOwner::new),
        |value| {
            // Each completed child owns one temporary root. A container consumes
            // those roots when its argument or result takes ownership of the children.
            let mark = key_roots.map(|_| {
                let children = match &value {
                    DecodedJson::Array(items) => items.len(),
                    DecodedJson::Object(fields) => fields.len(),
                    _ => 0,
                };
                ctx.heap.temporary_mark() - children
            });
            let mut released = None;
            let result = match value {
                DecodedJson::Null => NONE,
                DecodedJson::Bool(true) => TRUE,
                DecodedJson::Bool(false) => FALSE,
                DecodedJson::Number(JsonNumber::Integer(n)) => ctx.alloc(Value::Int(n)),
                DecodedJson::Number(JsonNumber::Float(n)) => ctx.alloc(Value::Float(n)),
                DecodedJson::Constant(name) => {
                    let name = ctx.text(name);
                    let value = ctx.call(constant, vec![name], Vec::new())?;
                    ctx.heap.keep(value);
                    value
                }
                DecodedJson::String(body) => ParsedString::read(ctx, text, wide, body)?.boxed(ctx),
                DecodedJson::Array(items) => ctx.alloc(Value::List(items)),
                DecodedJson::Object(fields) => {
                    let mut entries = Vec::new();
                    let mut positions = HashMap::new();
                    let mut pairs = Vec::new();
                    let allocation = |_| crate::Error::allocation();
                    if pairs_hook == NONE {
                        entries.try_reserve(fields.len()).map_err(allocation)?;
                        positions.try_reserve(fields.len()).map_err(allocation)?;
                    } else {
                        pairs.try_reserve(fields.len()).map_err(allocation)?;
                    }
                    keys.try_reserve(fields.len()).map_err(allocation)?;
                    for (body, value) in fields {
                        let name = ParsedString::read(ctx, text, wide, body)?;
                        let key = match keys.entry(name) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let id = e.key().boxed_key(ctx)?;
                                if let Some(root) = key_roots {
                                    let Value::List(keys) = ctx.heap.get_mut(root) else {
                                        unreachable!()
                                    };
                                    keys.try_reserve(1).map_err(allocation)?;
                                    keys.push(id);
                                }
                                *e.insert(id)
                            }
                        };
                        if pairs_hook != NONE {
                            pairs.push(ctx.alloc(Value::Tuple(vec![key, value])));
                            continue;
                        }
                        let position = *positions.entry(key).or_insert(entries.len());
                        if position == entries.len() {
                            entries.push((key, value));
                        } else {
                            entries[position] = (key, value);
                        }
                    }
                    if let Some(mark) = mark {
                        let argument = ctx.alloc(Value::List(pairs));
                        ctx.heap.release_temporaries_since(mark);
                        ctx.heap.keep(argument);
                        let result = ctx.call(pairs_hook, vec![argument], Vec::new())?;
                        released = Some(argument);
                        result
                    } else {
                        ctx.dict(entries)
                    }
                }
            };
            if let Some(mark) = mark {
                let collect =
                    released.is_some_and(|arg| arg != result && observe_release(ctx, arg));
                ctx.heap.release_temporaries_since(mark);
                ctx.heap.keep(result);
                if collect {
                    ctx.collect_unreachable();
                }
            }
            Ok(result)
        },
    )
}

#[derive(Default)]
struct Output {
    text: String,
    /// Raw surrogates in order, each with its insertion offset in the UTF-8 text.
    surrogates: Vec<(usize, u32)>,
}

impl Output {
    fn quote(&mut self, value: &Value, ascii: bool) -> Result<()> {
        match value {
            Value::Str(text) => write_json_string(&mut self.text, text, ascii)?,
            Value::SurrogateStr(points) => {
                write_json_codepoints(&mut self.text, &mut self.surrogates, points, ascii)?;
            }
            _ => unreachable!("validated string storage"),
        }
        Ok(())
    }

    /// Append an already formatted Python string, retaining unpaired surrogates.
    fn raw(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Str(text) => write_json_fragment(&mut self.text, text)?,
            Value::SurrogateStr(points) => {
                for &point in points {
                    if let Some(c) = char::from_u32(point) {
                        write_json_fragment(&mut self.text, c.encode_utf8(&mut [0; 4]))?;
                    } else {
                        self.surrogates
                            .try_reserve(1)
                            .map_err(|_| crate::Error::allocation())?;
                        self.surrogates.push((self.text.len(), point));
                    }
                }
            }
            _ => unreachable!("validated string storage"),
        }
        Ok(())
    }

    fn finish(self, ctx: &mut Context) -> Result<Id> {
        if self.surrogates.is_empty() {
            return Ok(ctx.text(self.text));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(
                self.text
                    .chars()
                    .count()
                    .saturating_add(self.surrogates.len()),
            )
            .map_err(|_| crate::Error::allocation())?;
        let mut previous = 0;
        for (offset, point) in self.surrogates {
            points.extend(self.text[previous..offset].chars().map(u32::from));
            points.push(point);
            previous = offset;
        }
        points.extend(self.text[previous..].chars().map(u32::from));
        Ok(ctx.alloc(Value::SurrogateStr(points)))
    }
}

fn string_argument(ctx: &Context, input: Id) -> Result<Id> {
    ctx.string_storage(input).ok_or_else(|| {
        let name = ctx.type_name(input);
        let name = &name[..name.floor_char_boundary(80)];
        Error::new(
            "TypeError",
            format!("first argument must be a string, not {name}"),
        )
    })
}

/// Quote a Python string with the native JSON encoder's escaping rules.
pub(super) fn encode_basestring(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((_, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("{name} requires a module receiver"),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    let [input] = args else {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes exactly one argument ({} given)", args.len()),
        ));
    };
    let storage = string_argument(ctx, *input)?;
    let mut output = Output::default();
    output.quote(
        ctx.heap.get(storage),
        name == "_json.encode_basestring_ascii",
    )?;
    output.finish(ctx)
}

/// Return the string storage used for a JSON encoder separator.
fn separator_storage(ctx: &mut Context, value: Id, position: usize) -> Result<Id> {
    if let Some(storage) = ctx.string_storage(value) {
        return Ok(storage);
    }
    let class = ctx.exact_type(value)?;
    let name = if value == NONE {
        "None"
    } else {
        ctx.class_name(class)
    };
    Err(Error::new(
        "TypeError",
        format!(
            "make_encoder() argument {position} must be str, not {}",
            &name[..name.floor_char_boundary(50)]
        ),
    ))
}

struct Encoder {
    ascii: bool,
    quote: Option<Id>,
    allow_nan: Id,
    indent: Option<Id>,
    indent_prefix: Option<Id>,
    sort_keys: Id,
    skipkeys: Id,
    item_separator: Id,
    key_separator: Id,
    default: Id,
    markers: Option<Id>,
    nesting: usize,
    output: Output,
}

/// Invoke a constructed _json.Encoder with the original object and starting level.
pub(super) fn encoder_call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let bad_state = || Error::new("TypeError", "invalid JSON encoder state");
    let (state, positional) = args.split_first().ok_or_else(bad_state)?;
    let Value::Tuple(fields) = ctx.heap.get(*state) else {
        return Err(bad_state());
    };
    let &[
        markers,
        default,
        quote,
        indent,
        key,
        item,
        sort_keys,
        skipkeys,
        allow_nan,
    ] = fields.as_slice()
    else {
        return Err(bad_state());
    };
    let argument = required_args(
        "_iterencode",
        positional,
        kwargs,
        &["obj", "_current_indent_level"],
    )?;
    let value = argument(0)?;
    let level = ctx
        .index_integer(argument(1)?)?
        .to_i64()
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    let marker_dict = if markers == NONE {
        None
    } else {
        Some(ctx.dict_storage(markers).ok_or_else(bad_state)?)
    };
    let key_separator = separator_storage(ctx, key, 5)?;
    let item_separator = separator_storage(ctx, item, 6)?;
    // CPython repeats the original indent before visiting even a scalar. Subsequent
    // levels concatenate its underlying text rather than repeating the subclass.
    let indent_prefix = if level == 0 || indent == NONE {
        None
    } else {
        let sequence = matches!(
            ctx.heap.get(ctx.scalar_value(indent)),
            Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::List(_)
                | Value::Tuple(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
        );
        if !sequence
            && (ctx.dict_storage(indent).is_some()
                || ctx.bound_slot(indent, "__getitem__")?.is_none()
                || ctx.bound_slot(indent, "__mul__")?.is_none())
        {
            return Err(Error::new(
                "TypeError",
                format!("'{}' object can't be repeated", ctx.type_name(indent)),
            ));
        }
        let count = ctx.int(level as i64);
        let repeated = ctx.binary("*", indent, count)?;
        Some(
            ctx.string_storage(repeated)
                .ok_or_else(|| Error::new("SystemError", "bad argument to internal function"))?,
        )
    };
    let indent = (indent != NONE).then(|| ctx.string_storage(indent).unwrap_or(indent));
    let symbol = if let Value::Method {
        callable,
        receiver,
        explicit: false,
    } = *ctx.heap.get(quote)
        && matches!(ctx.heap.get(receiver), Value::Module { name, .. } if name == "_json")
        && let Value::Native(name) = ctx.heap.get(callable)
    {
        Some(name.as_str())
    } else {
        None
    };
    let (ascii, quote) = match symbol {
        Some("_json.encode_basestring_ascii") => (true, None),
        Some("_json.encode_basestring") => (false, None),
        _ => (false, Some(quote)),
    };
    let mut encoder = Encoder {
        ascii,
        quote,
        allow_nan,
        indent,
        sort_keys,
        skipkeys,
        item_separator,
        key_separator,
        default,
        markers: marker_dict,
        indent_prefix,
        nesting: 0,
        output: Output::default(),
    };
    encoder.value(ctx, value, 0)?;
    let text = encoder.output.finish(ctx)?;
    Ok(ctx.alloc(Value::Tuple(vec![text])))
}

/// Exact JSON scalars have no Python callbacks or child references.
fn json_scalar(ctx: &Context, id: Id) -> bool {
    matches!(
        ctx.heap.get(id),
        Value::None
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Str(_)
            | Value::SurrogateStr(_)
    )
}

fn scalar_entries(ctx: &Context, entries: &[(Id, Id)]) -> bool {
    entries
        .iter()
        .all(|&(key, value)| json_scalar(ctx, key) && json_scalar(ctx, value))
}

/// Prove a small replacement contains only plain JSON data. Charge repeated
/// references against the same budget so cycles and shared subgraphs stay bounded.
fn plain_replacement(ctx: &Context, id: Id, budget: &mut usize) -> bool {
    let Some(next) = budget.checked_sub(1) else {
        return false;
    };
    *budget = next;
    match ctx.heap.get(id) {
        Value::Tuple(items) | Value::List(items) => items
            .iter()
            .all(|&item| plain_replacement(ctx, item, budget)),
        Value::Dict(entries) => {
            entries.order_len() == 0
                && entries.iter().all(|&(key, value)| {
                    plain_replacement(ctx, key, budget) && plain_replacement(ctx, value, budget)
                })
        }
        _ => json_scalar(ctx, id),
    }
}

/// Test whether dropping an encoder callback result can run observable cleanup.
fn observe_release(ctx: &Context, result: Id) -> bool {
    if !ctx.heap.observes_released_roots() {
        return false;
    }
    if let Value::Dict(entries) = ctx.heap.get(result) {
        // An ordered node can still own a key removed through a dict descriptor.
        if entries.order_len() != 0 {
            return true;
        }
        if scalar_entries(ctx, entries) {
            return false;
        }
    }
    ctx.heap.could_release_callback_result(result) && !plain_replacement(ctx, result, &mut 64)
}

impl Encoder {
    /// Keep the object in the caller's marker dictionary until its encoding succeeds.
    fn mark(&self, ctx: &mut Context, object: Id) -> Result<Option<(Id, Id)>> {
        let Some(dict) = self.markers else {
            return Ok(None);
        };
        let key = ctx.alloc(Value::Int(crate::Int::from_usize(object as usize)));
        let (hash, found) = ctx.find_dictionary_key(dict, key, None)?;
        if found.is_some() {
            return Err(Error::new("ValueError", "Circular reference detected"));
        }
        ctx.set_dictionary_key(dict, key, object, Some(hash), true)?;
        Ok(Some((dict, key)))
    }

    /// Call the selected string encoder on the original Python object.
    fn string(&mut self, ctx: &mut Context, id: Id) -> Result<()> {
        let Some(quote) = self.quote else {
            return self
                .output
                .quote(ctx.heap.get(ctx.scalar_value(id)), self.ascii);
        };
        let roots = ctx.heap.temporary_mark();
        let encoded = ctx.call(quote, super::super::copy_ids(&[id])?, Vec::new())?;
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(encoded);
        let outcome = if let Some(storage) = ctx.string_storage(encoded) {
            self.output.raw(ctx.heap.get(storage))
        } else {
            ctx.exact_type(encoded).and_then(|class| {
                let name = ctx.class_name(class);
                let name = &name[..name.floor_char_boundary(80)];
                Err(Error::new(
                    "TypeError",
                    format!("encoder() must return a string, not {name}"),
                ))
            })
        };
        Self::release_encoded(ctx, roots, encoded, outcome)
            .map_err(|exc| notes::live_error(ctx, exc))
    }

    fn value(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.value_inner(ctx, id, depth)
        })
    }

    fn value_inner(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        match ctx.heap.get(ctx.scalar_value(id)) {
            Value::None => write_json_fragment(&mut self.output.text, "null")?,
            Value::Bool(value) => {
                write_json_fragment(&mut self.output.text, if *value { "true" } else { "false" })?
            }
            Value::Int(value) => write_json_fragment(
                &mut self.output.text,
                &crate::integer_text::decimal_string(value)?,
            )?,
            Value::Float(value) => write_json_fragment(
                &mut self.output.text,
                &float_text(ctx, id, *value, self.allow_nan)?,
            )?,
            Value::Str(_) | Value::SurrogateStr(_) => self.string(ctx, id)?,
            _ => {
                if self.nesting >= 512 {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum JSON nesting exceeded",
                    ));
                }
                let roots = ctx.heap.temporary_mark();
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(id);
                let sequence =
                    ctx.builtin_slot_accepts(id, "list") || ctx.builtin_slot_accepts(id, "tuple");
                // A recursive subclass can end iteration by returning an empty snapshot.
                if sequence && matches!(ctx.heap.get(id), Value::Instance { .. }) {
                    let snapshot = Self::snapshot(ctx, id, None)?;
                    let result = self.write_nested(ctx, id, depth, sequence, snapshot);
                    return Self::release_encoded(ctx, roots, snapshot, result)
                        .map_err(|exc| notes::live_error(ctx, exc));
                }
                self.write_nested(ctx, id, depth, sequence, id)?;
                ctx.heap.release_temporaries_since(roots);
            }
        }
        Ok(())
    }

    fn write_nested(
        &mut self,
        ctx: &mut Context,
        id: Id,
        depth: usize,
        sequence: bool,
        contents: Id,
    ) -> Result<()> {
        let empty = match ctx.heap.get(contents) {
            Value::List(items) | Value::Tuple(items) | Value::NamedTuple { values: items, .. }
                if items.is_empty() =>
            {
                Some("[]")
            }
            _ if ctx.dict_entries(id).is_some_and(<[_]>::is_empty) => Some("{}"),
            _ => None,
        };
        if let Some(empty) = empty {
            write_json_fragment(&mut self.output.text, empty)?;
            return Ok(());
        }
        let marker = self.mark(ctx, id)?;
        // The accelerator prepares its separator before visiting any
        // items, even if every key of the nonempty dict will be skipped.
        if let Some(indent) = self.indent
            && (sequence || ctx.dict_storage(id).is_some())
            && ctx.string_storage(indent).is_none()
        {
            let class = ctx.exact_type(indent)?;
            let name = ctx.class_name(class);
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only concatenate str (not \"{}\") to str",
                    &name[..name.floor_char_boundary(200)]
                ),
            ));
        }
        self.nesting += 1;
        match ctx.heap.get(contents) {
            Value::List(_) | Value::Tuple(_) | Value::NamedTuple { .. } => {
                write_json_fragment(&mut self.output.text, "[")?;
                let mut i = 0;
                while let Value::List(items)
                | Value::Tuple(items)
                | Value::NamedTuple { values: items, .. } = ctx.heap.get(contents)
                {
                    let Some(&item) = items.get(i) else { break };
                    self.separator(ctx, i != 0, depth + 1)?;
                    self.value(ctx, item, depth + 1)
                        .map_err(|e| noted(ctx, e, id, Location::Index(i)))?;
                    i += 1;
                }
                self.close(ctx, "]", i != 0, depth)?;
            }
            Value::Dict(_) | Value::DefaultDict { .. } => self.object(ctx, id, depth)?,
            Value::Instance {
                dictionary: Some(_),
                ..
            } => self.object_items(ctx, id, depth)?,
            _ => {
                let mark = ctx.heap.temporary_mark();
                let result = ctx.call(self.default, super::super::copy_ids(&[id])?, Vec::new())?;
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(result);
                let encoded = self.value(ctx, result, depth);
                // Release the replacement before formatting the object's error note.
                Self::release_encoded(ctx, mark, result, encoded)
                    .map_err(|exc| notes::noted_instance(ctx, exc, id, Location::Object))?;
            }
        }
        self.nesting -= 1;
        if let Some((dict, key)) = marker
            && super::builtins::remove_dict_item(ctx, dict, key)?.is_none()
        {
            return Err(ctx.exception("KeyError", &[key]));
        }
        Ok(())
    }

    /// Drop a callback's intermediate value before the next member. A finalizer can
    /// mutate both the enclosing container and the exception being propagated.
    fn release_encoded(
        ctx: &mut Context,
        mark: usize,
        value: Id,
        outcome: Result<()>,
    ) -> std::result::Result<(), Id> {
        let collect = observe_release(ctx, value);
        let exception = outcome.err().map(|error| ctx.raised_instance(error));
        ctx.heap.release_temporaries_since(mark);
        if let Some(exception) = exception {
            ctx.heap.keep(exception);
        }
        if collect {
            ctx.collect_unreachable();
        }
        exception.map_or(Ok(()), Err)
    }

    /// Write a separator and the indentation before a sequence item or object member.
    fn separator(&mut self, ctx: &Context, after_first: bool, depth: usize) -> Result<()> {
        if after_first {
            self.output.raw(ctx.heap.get(self.item_separator))?;
        }
        self.newline(ctx, depth)
    }

    /// Align a nonempty container's closing delimiter with its opening level.
    fn close(
        &mut self,
        ctx: &Context,
        delimiter: &str,
        nonempty: bool,
        depth: usize,
    ) -> Result<()> {
        if nonempty {
            self.newline(ctx, depth)?;
        }
        write_json_fragment(&mut self.output.text, delimiter)?;
        Ok(())
    }

    fn newline(&mut self, ctx: &Context, depth: usize) -> Result<()> {
        if let Some(indent) = self.indent {
            if self.indent_prefix.is_none()
                && let Value::Str(text) = ctx.heap.get(indent)
            {
                crate::objects::JsonIndent::Text(text)
                    .write_newline(&mut self.output.text, depth)?;
            } else {
                write_json_fragment(&mut self.output.text, "\n")?;
                if let Some(prefix) = self.indent_prefix {
                    self.output.raw(ctx.heap.get(prefix))?;
                }
                for _ in 0..depth {
                    self.output.raw(ctx.heap.get(indent))?;
                }
            }
        }
        Ok(())
    }

    fn object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let entries = ctx.dict_entries(id).expect("dictionary storage");
        if matches!(ctx.heap.get(id), Value::DefaultDict { .. }) {
            return self.object_items(ctx, id, depth);
        }
        if self.quote.is_some() || !scalar_entries(ctx, entries) {
            return if self.sort_keys == FALSE {
                self.live_object(ctx, id, depth)
            } else {
                self.object_items(ctx, id, depth)
            };
        }
        let mut entries = super::super::copy_ids(entries)?;
        // Encoding a nested subclass can call Python and clear the original mapping.
        ctx.heap
            .reserve_temporaries(entries.len().saturating_mul(2))?;
        for &(key, value) in &entries {
            ctx.heap.keep(key);
            ctx.heap.keep(value);
        }
        if ctx.truth(self.sort_keys)? {
            super::builtins::sort_values(ctx, &mut entries, false, |(key, _)| key)?;
        }
        write_json_fragment(&mut self.output.text, "{")?;
        let mut nonempty = false;
        for (key, value) in entries {
            self.member(ctx, id, &mut nonempty, key, value, depth + 1)?;
        }
        self.close(ctx, "}", nonempty, depth)
    }

    /// A Python callback may change subsequent members of an unsorted exact dict.
    fn live_object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        write_json_fragment(&mut self.output.text, "{")?;
        let mut nonempty = false;
        let mut position = 0;
        while let Some((current, key, value, _)) = ctx.dictionary_pair(id, position) {
            let roots = ctx.heap.temporary_mark();
            ctx.heap.reserve_temporaries(2)?;
            ctx.heap.keep(key);
            ctx.heap.keep(value);
            self.member(ctx, id, &mut nonempty, key, value, depth + 1)?;
            ctx.heap.release_temporaries_since(roots);
            position = current + 1;
        }
        self.close(ctx, "}", nonempty, depth)
    }

    /// Materialize a subclass iterator while keeping previously yielded values alive.
    fn snapshot(ctx: &mut Context, source: Id, items_owner: Option<Id>) -> Result<Id> {
        let iterator = ctx.iterator(source).map_err(|error| {
            let ty = ctx.builtin("TypeError");
            if !ctx.raised_error_matches(&error, ty) {
                return error;
            }
            if let Some(owner) = items_owner {
                let owner = super::diagnostic_type_name(ctx, owner);
                let result = super::diagnostic_type_name(ctx, source);
                Error::new(
                    "TypeError",
                    format!(
                        "{}.items() returned a non-iterable (type {})",
                        &owner[..owner.floor_char_boundary(200)],
                        &result[..result.floor_char_boundary(200)],
                    ),
                )
            } else {
                Error::new("TypeError", "_iterencode_list needs a sequence")
            }
        })?;
        let list = ctx.alloc(Value::List(Vec::new()));
        super::builtins::call(ctx, "list.extend", &[list, iterator], &[])?;
        Ok(list)
    }

    /// Read the mapping through its items method and sort the returned pairs when requested.
    fn object_items(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let mark = ctx.heap.temporary_mark();
        let method = ctx.lookup_attr(id, "items")?;
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        let items = if matches!(ctx.heap.get(result), Value::List(_)) {
            result
        } else {
            Self::snapshot(ctx, result, Some(id))?
        };
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(items);
        let result = self.write_pairs(ctx, id, depth, items);
        // The C encoder releases the mapping's items while its marker is still set.
        Self::release_encoded(ctx, mark, items, result).map_err(|exc| notes::live_error(ctx, exc))
    }

    fn write_pairs(&mut self, ctx: &mut Context, id: Id, depth: usize, items: Id) -> Result<()> {
        if ctx.truth(self.sort_keys)? {
            super::builtins::call(ctx, "list.sort", &[items], &[])?;
        }
        write_json_fragment(&mut self.output.text, "{")?;
        let mut i = 0;
        let mut nonempty = false;
        while let Value::List(pairs) = ctx.heap.get(items) {
            let Some(&pair) = pairs.get(i) else { break };
            let values = ctx.tuple_items(pair)?;
            let Some(&[key, value]) = values.as_deref() else {
                return Err(Error::new("ValueError", "items must return 2-tuples"));
            };
            // The callback may remove its pair from the list returned by items().
            let mark = ctx.heap.temporary_mark();
            ctx.heap.reserve_temporaries(2)?;
            ctx.heap.keep(key);
            ctx.heap.keep(value);
            self.member(ctx, id, &mut nonempty, key, value, depth + 1)?;
            ctx.heap.release_temporaries_since(mark);
            i += 1;
        }
        self.close(ctx, "}", nonempty, depth)
    }

    /// Write an object member after any sorting and pair validation.
    fn member(
        &mut self,
        ctx: &mut Context,
        owner: Id,
        nonempty: &mut bool,
        key: Id,
        value: Id,
        depth: usize,
    ) -> Result<()> {
        let converted = match ctx.heap.get(ctx.scalar_value(key)) {
            Value::Str(_) | Value::SurrogateStr(_) => None,
            Value::Int(n) => Some(crate::integer_text::decimal_string(n)?),
            Value::Float(n) => Some(float_text(ctx, key, *n, self.allow_nan)?),
            Value::None => Some("null".into()),
            Value::Bool(true) => Some("true".into()),
            Value::Bool(false) => Some("false".into()),
            _ => {
                if ctx.truth(self.skipkeys)? {
                    return Ok(());
                }
                // The one-shot encoder reports up to 100 bytes of the actual type name.
                let class = ctx.exact_type(key)?;
                let name = ctx.class_name(class);
                let name = ctx.text(name[..name.floor_char_boundary(100)].to_owned());
                let prefix = ctx.text("keys must be str, int, float, bool or None, not ");
                let message = ctx.concat_strings(prefix, name)?;
                return Err(ctx.exception("TypeError", &[message]));
            }
        };
        self.separator(ctx, *nonempty, depth)?;
        *nonempty = true;
        if let Some(text) = converted {
            if self.quote.is_some() {
                let mark = ctx.heap.temporary_mark();
                let string = match key {
                    // CPython reuses the constant names but renders numeric keys afresh.
                    NONE | TRUE | FALSE => ctx.heap.string_literal(&text)?,
                    _ => ctx.heap.fresh_string(Value::Str(text))?,
                };
                self.string(ctx, string)?;
                ctx.heap.release_temporaries_since(mark);
            } else {
                write_json_string(&mut self.output.text, &text, self.ascii)?;
            }
        } else {
            self.string(ctx, key)?;
        }
        self.output.raw(ctx.heap.get(self.key_separator))?;
        self.value(ctx, value, depth)
            .map_err(|e| noted(ctx, e, owner, Location::Key(key)))
    }
}

/// Spell nonfinite values as JSON extensions unless strict encoding rejects them.
fn float_text(ctx: &mut Context, id: Id, value: f64, allow_nan: Id) -> Result<Str> {
    if value.is_finite() {
        return Ok(crate::floats::render(value));
    }
    if !ctx.truth(allow_nan)? {
        let repr = ctx.builtin("repr");
        let text = ctx.call(repr, super::super::copy_ids(&[id])?, Vec::new())?;
        return Err(Error::new(
            "ValueError",
            format!(
                "Out of range float values are not JSON compliant: {}",
                ctx.str_value(text)?
            ),
        ));
    }
    Ok(if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else {
        "-Infinity".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_nested_objects_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let argv = [c"imre".as_ptr()];
                // SAFETY: argv and the configuration are valid for the initializer,
                // which transfers ownership of a newly allocated context.
                let mut ctx = unsafe {
                    Box::from_raw(crate::dynamic::ffi::imre_dyn_new(
                        1,
                        argv.as_ptr(),
                        c"{\"paths\":[]}".as_ptr(),
                    ))
                };
                let key = ctx.text("x");
                let mut value = NONE;
                for _ in 0..400 {
                    value = ctx.dict(vec![(key, value)]);
                }
                let json = ctx.module("_json").unwrap();
                let quote = ctx.lookup_attr(json, "encode_basestring_ascii").unwrap();
                let colon = ctx.text(": ");
                let comma = ctx.text(", ");
                let options = [NONE, NONE, quote, NONE, colon, comma, FALSE, FALSE, TRUE];
                let state = encoder_state(&mut ctx, &options, &[]).unwrap();
                let zero = ctx.int(0);
                let chunks = encoder_call(&mut ctx, &[state, value, zero], &[]).unwrap();
                let output = ctx.get_item(chunks, zero).unwrap();
                assert_eq!(
                    ctx.str_value(output).unwrap(),
                    format!("{}null{}", "{\"x\": ".repeat(400), "}".repeat(400))
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
