mod encoder_state;
mod notes;
mod scanstring;
pub(super) use encoder_state::encoder_state;
use notes::Location;
pub(super) use scanstring::scanstring;

use std::collections::{HashMap, HashSet};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::objects::{
    DecodedJson, JsonNumber, decode_json, decode_json_bytes, owned_bytes_input,
    write_json_codepoints, write_json_fragment, write_json_string,
};
use crate::{Str, StrOwner};

pub(super) fn loads(ctx: &mut Context, input: Id, raise_error: Option<Id>) -> Result<Id> {
    let result = match ctx.heap.get(input) {
        Value::Str(text) => {
            let text = Str::try_new(text).map_err(|_| crate::Error::allocation())?;
            if text.starts_with('\u{feff}') {
                let document = StrOwner::try_new(text).map_err(|_| crate::Error::allocation())?;
                let error = crate::objects::json_bom_error(document);
                return Err(decode_error(ctx, input, raise_error, true, error));
            }
            parse(ctx, &text)
        }
        Value::Bytes(bytes) => {
            let owner = owned_bytes_input(bytes)?;
            parse(ctx, &decode_json_bytes(&owner)?)
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "the JSON object must be str, bytes or bytearray, not {}",
                    ctx.type_name(input)
                ),
            ));
        }
    };
    result.map_err(|error| decode_error(ctx, input, raise_error, false, error))
}

/// Raise the class owned by the calling package, retaining the Python document's identity.
fn decode_error(
    ctx: &mut Context,
    input: Id,
    raise_error: Option<Id>,
    bom: bool,
    error: crate::Error,
) -> Error {
    let (Some(callback), Some((message, document, position))) =
        (raise_error, error.json_decode_fields())
    else {
        return error.into();
    };
    let message = ctx.text(message);
    let document = if matches!(ctx.heap.get(input), Value::Str(_)) {
        input
    } else {
        ctx.text(document)
    };
    let position = ctx.int(position as i64);
    let bom = if bom { TRUE } else { FALSE };
    ctx.call(callback, vec![message, document, position, bom], Vec::new())
        .err()
        .unwrap_or_else(|| Error::new("SystemError", "JSON error callback returned"))
}

fn parse(ctx: &mut Context, text: &str) -> std::result::Result<Id, crate::Error> {
    let mut keys = HashMap::new();
    decode_json(
        text,
        || Str::try_new(text).ok().map(StrOwner::new),
        |value| {
            Ok(match value {
                DecodedJson::Null => NONE,
                DecodedJson::Bool(true) => TRUE,
                DecodedJson::Bool(false) => FALSE,
                DecodedJson::Number(JsonNumber::Integer(n)) => ctx.alloc(Value::Int(n)),
                DecodedJson::Number(JsonNumber::Float(n)) => ctx.alloc(Value::Float(n)),
                DecodedJson::String(s) => ctx.text(s.to_string()),
                DecodedJson::Array(items) => ctx.alloc(Value::List(items)),
                DecodedJson::Object(fields) => {
                    let mut entries = Vec::new();
                    let mut positions = HashMap::new();
                    let allocation = |_| crate::Error::allocation();
                    entries.try_reserve(fields.len()).map_err(allocation)?;
                    positions.try_reserve(fields.len()).map_err(allocation)?;
                    keys.try_reserve(fields.len()).map_err(allocation)?;
                    for (name, value) in fields {
                        let key = *keys
                            .entry(name)
                            .or_insert_with_key(|s| ctx.text(s.to_string()));
                        let position = *positions.entry(key).or_insert(entries.len());
                        if position == entries.len() {
                            entries.push((key, value));
                        } else {
                            entries[position] = (key, value);
                        }
                    }
                    ctx.dict(entries)
                }
            })
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

enum Fragment {
    Text(String),
    Python(Id),
}

impl Fragment {
    fn separator(ctx: &mut Context, value: Id, position: usize) -> Result<Self> {
        if let Some(storage) = ctx.string_storage(value) {
            return Ok(Self::Python(storage));
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

    fn text<'a>(&'a self, ctx: &'a Context) -> Option<&'a str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Python(id) => match ctx.heap.get(*id) {
                Value::Str(text) => Some(text),
                _ => None,
            },
        }
    }

    fn validate_indent(&self, ctx: &mut Context) -> Result<()> {
        if let Self::Python(id) = self
            && ctx.string_storage(*id).is_none()
        {
            let class = ctx.exact_type(*id)?;
            let name = ctx.class_name(class);
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only concatenate str (not \"{}\") to str",
                    &name[..name.floor_char_boundary(200)]
                ),
            ));
        }
        Ok(())
    }

    fn write(&self, ctx: &Context, output: &mut Output) -> Result<()> {
        match self {
            Self::Text(text) => write_json_fragment(&mut output.text, text)?,
            Self::Python(id) => output.raw(ctx.heap.get(*id))?,
        }
        Ok(())
    }
}

struct Encoder {
    ascii: bool,
    quote: Option<Id>,
    allow_nan: Id,
    indent: Option<Fragment>,
    indent_prefix: Option<Id>,
    sort_keys: Id,
    skipkeys: Id,
    item_separator: Fragment,
    key_separator: Fragment,
    default: Option<Id>,
    stream: Option<Id>,
    write_failed: bool,
    markers: Markers,
    nesting: usize,
    output: Output,
}

/// Encode to an owned string or write fragments to a Python text stream.
pub(super) fn encode(
    ctx: &mut Context,
    id: Id,
    kwargs: &[(String, Id)],
    stream: Option<Id>,
) -> Result<Id> {
    let option = |name| {
        kwargs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| *value)
    };
    let mut skipkeys = option("skipkeys").unwrap_or(FALSE);
    let mut ascii = option("ensure_ascii").unwrap_or(TRUE);
    let mut allow_nan = option("allow_nan").unwrap_or(TRUE);
    let mut sort_keys = option("sort_keys").unwrap_or(FALSE);
    let indent_option = option("indent").unwrap_or(NONE);
    // The public functions test these options in order when choosing the
    // cached default encoder. That encoder does not retain the caller's flags.
    if !ctx.truth(skipkeys)?
        && ctx.truth(ascii)?
        && ctx.truth(allow_nan)?
        && indent_option == NONE
        && option("separators").unwrap_or(NONE) == NONE
        && option("default").unwrap_or(NONE) == NONE
        && !ctx.truth(sort_keys)?
    {
        (skipkeys, ascii, allow_nan, sort_keys) = (FALSE, TRUE, TRUE, FALSE);
    }
    let (item_separator, key_separator) = match option("separators") {
        None | Some(NONE) => (
            (if indent_option != NONE { "," } else { ", " }).to_owned(),
            ": ".to_owned(),
        ),
        Some(separators) => {
            let parts = ctx.items(separators)?;
            let [item, key] = parts.as_slice() else {
                return Err(Error::new(
                    "ValueError",
                    "separators must contain two items",
                ));
            };
            (
                ctx.str_value(*item)?.to_owned(),
                ctx.str_value(*key)?.to_owned(),
            )
        }
    };
    let ascii = ctx.truth(ascii)?;
    let string_only = stream.is_none() && ctx.string_storage(id).is_some();
    let indent = if indent_option == NONE || string_only {
        None
    } else {
        let unit = if ctx.str_value(indent_option).is_ok() {
            indent_option
        } else {
            let space = ctx.text(" ");
            ctx.binary("*", space, indent_option)?
        };
        Some(ctx.str_value(unit)?.to_owned())
    };
    if stream.is_none() && !string_only {
        // The one-shot encoder snapshots these flags when it is constructed.
        // The streaming encoder tests each flag only where it is used.
        sort_keys = super::builtins::boolean(ctx.truth(sort_keys)?);
        skipkeys = super::builtins::boolean(ctx.truth(skipkeys)?);
        allow_nan = super::builtins::boolean(ctx.truth(allow_nan)?);
    }
    let mut encoder = Encoder {
        ascii,
        quote: None,
        allow_nan,
        indent: indent.map(Fragment::Text),
        sort_keys,
        skipkeys,
        item_separator: Fragment::Text(item_separator),
        key_separator: Fragment::Text(key_separator),
        default: option("default").filter(|v| *v != NONE),
        stream,
        write_failed: false,
        markers: Markers::Local(HashSet::new()),
        indent_prefix: None,
        nesting: 0,
        output: Output::default(),
    };
    encoder.value(ctx, id, 0)?;
    Ok(if stream.is_some() {
        NONE
    } else {
        encoder.output.finish(ctx)?
    })
}

/// The public factory retains the caller's marker dictionary across calls. CPython
/// removes completed entries and leaves unfinished entries visible after an error.
enum Markers {
    Local(HashSet<Id>),
    External(Option<Id>),
}

impl Markers {
    fn enter(&mut self, ctx: &mut Context, object: Id) -> Result<Option<Id>> {
        match self {
            Self::External(None) => return Ok(None),
            Self::Local(active) => {
                if active.insert(object) {
                    return Ok(None);
                }
            }
            Self::External(Some(dict)) => {
                let key = ctx.alloc(Value::Int(crate::Int::from_usize(object as usize)));
                let (hash, found) = ctx.find_dictionary_key(*dict, key, None)?;
                if found.is_none() {
                    ctx.set_dictionary_key(*dict, key, object, Some(hash), true)?;
                    return Ok(Some(key));
                }
            }
        }
        Err(Error::new("ValueError", "Circular reference detected"))
    }

    fn leave(&mut self, ctx: &mut Context, object: Id, key: Option<Id>) -> Result<()> {
        match self {
            Self::Local(active) => {
                active.remove(&object);
            }
            Self::External(Some(dict)) => {
                let key = key.expect("marker inserted on entry");
                if super::builtins::remove_dict_item(ctx, *dict, key)?.is_none() {
                    return Err(ctx.exception("KeyError", &[key]));
                }
            }
            Self::External(None) => {}
        }
        Ok(())
    }
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
    let argument = encoder_state::required_args(
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
    let key_separator = Fragment::separator(ctx, key, 5)?;
    let item_separator = Fragment::separator(ctx, item, 6)?;
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
    let indent =
        (indent != NONE).then(|| Fragment::Python(ctx.string_storage(indent).unwrap_or(indent)));
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
        default: Some(default),
        stream: None,
        write_failed: false,
        markers: Markers::External(marker_dict),
        indent_prefix,
        nesting: 0,
        output: Output::default(),
    };
    encoder.value(ctx, value, 0)?;
    let text = encoder.output.finish(ctx)?;
    Ok(ctx.alloc(Value::Tuple(vec![text])))
}

impl Encoder {
    /// Deliver each completed fragment before running later user code.
    fn emit(&mut self, ctx: &mut Context) -> Result<()> {
        if let Some(stream) = self.stream
            && !self.output.text.is_empty()
        {
            let roots = ctx.heap.temporary_mark();
            let chunk = std::mem::take(&mut self.output).finish(ctx)?;
            let result = ctx
                .lookup_attr(stream, "write")
                .and_then(|write| ctx.call(write, super::super::copy_ids(&[chunk])?, Vec::new()));
            if let Err(error) = result {
                self.write_failed = true;
                return Err(error);
            }
            ctx.heap.release_temporaries_since(roots);
        }
        Ok(())
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
        let Some(storage) = ctx.string_storage(encoded) else {
            let class = ctx.exact_type(encoded)?;
            let name = ctx.class_name(class);
            let name = &name[..name.floor_char_boundary(80)];
            return Err(Error::new(
                "TypeError",
                format!("encoder() must return a string, not {name}"),
            ));
        };
        self.output.raw(ctx.heap.get(storage))?;
        ctx.heap.release_temporaries_since(roots);
        Ok(())
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
                self.emit(ctx)?;
                let sequence =
                    ctx.builtin_slot_accepts(id, "list") || ctx.builtin_slot_accepts(id, "tuple");
                let streaming =
                    self.stream.is_some() && (sequence || ctx.dict_entries(id).is_some());
                let contents =
                    if !streaming && sequence && matches!(ctx.heap.get(id), Value::Instance { .. })
                    {
                        Self::snapshot(ctx, id)?
                    } else {
                        id
                    };
                // Materialize subclass iteration before detecting a repeated container. A
                // second call may return an empty sequence and end the recursion itself.
                let empty = match ctx.heap.get(contents) {
                    _ if streaming => {
                        (!ctx.truth(id)?).then_some(if sequence { "[]" } else { "{}" })
                    }
                    Value::List(items)
                    | Value::Tuple(items)
                    | Value::NamedTuple { values: items, .. }
                        if items.is_empty() =>
                    {
                        Some("[]")
                    }
                    _ if ctx.dict_entries(id).is_some_and(<[_]>::is_empty) => Some("{}"),
                    _ => None,
                };
                if let Some(empty) = empty {
                    write_json_fragment(&mut self.output.text, empty)?;
                    ctx.heap.release_temporaries_since(roots);
                    return self.emit(ctx);
                }
                let marker = self.markers.enter(ctx, id)?;
                // The accelerator prepares its separator before visiting any
                // items, even if every key of the nonempty dict will be skipped.
                if let Some(indent) = &self.indent
                    && (sequence || ctx.dict_storage(id).is_some())
                {
                    indent.validate_indent(ctx)?;
                }
                self.nesting += 1;
                match ctx.heap.get(contents) {
                    _ if streaming && sequence => self.stream_array(ctx, id, depth)?,
                    _ if streaming => self.stream_object(ctx, id, depth)?,
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
                                .map_err(|e| self.noted(ctx, e, id, Location::Index(i)))?;
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
                        let default = self.default.ok_or_else(|| {
                            Error::new(
                                "TypeError",
                                format!(
                                    "Object of type {} is not JSON serializable",
                                    ctx.type_name(id)
                                ),
                            )
                        })?;
                        let result =
                            ctx.call(default, super::super::copy_ids(&[id])?, Vec::new())?;
                        self.value(ctx, result, depth)
                            .map_err(|e| self.noted(ctx, e, id, Location::Object))?;
                    }
                }
                self.nesting -= 1;
                self.markers.leave(ctx, id, marker)?;
                ctx.heap.release_temporaries_since(roots);
            }
        }
        self.emit(ctx)
    }

    /// Write a separator and the indentation before a sequence item or object member.
    fn separator(&mut self, ctx: &mut Context, after_first: bool, depth: usize) -> Result<()> {
        if after_first {
            self.item_separator.write(ctx, &mut self.output)?;
        }
        self.newline(ctx, depth)
    }

    /// Align a nonempty container's closing delimiter with its opening level.
    fn close(
        &mut self,
        ctx: &mut Context,
        delimiter: &str,
        nonempty: bool,
        depth: usize,
    ) -> Result<()> {
        if nonempty {
            self.newline(ctx, depth)?;
            self.emit(ctx)?;
        }
        write_json_fragment(&mut self.output.text, delimiter)?;
        Ok(())
    }

    fn newline(&mut self, ctx: &mut Context, depth: usize) -> Result<()> {
        if let Some(indent) = &self.indent {
            if self.indent_prefix.is_none()
                && let Some(text) = indent.text(ctx)
            {
                crate::objects::JsonIndent::Text(text)
                    .write_newline(&mut self.output.text, depth)?;
            } else {
                write_json_fragment(&mut self.output.text, "\n")?;
                if let Some(prefix) = self.indent_prefix {
                    self.output.raw(ctx.heap.get(prefix))?;
                }
                for _ in 0..depth {
                    indent.write(ctx, &mut self.output)?;
                }
            }
        }
        Ok(())
    }

    fn object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let entries = ctx.dict_entries(id).expect("dictionary storage");
        if matches!(self.markers, Markers::External(_)) {
            if matches!(ctx.heap.get(id), Value::DefaultDict { .. }) {
                return self.object_items(ctx, id, depth);
            }
            let scalar = |id| {
                matches!(
                    ctx.heap.get(id),
                    Value::None
                        | Value::Bool(_)
                        | Value::Int(_)
                        | Value::Float(_)
                        | Value::Str(_)
                        | Value::SurrogateStr(_)
                )
            };
            if self.quote.is_some() || !entries.iter().all(|&(k, v)| scalar(k) && scalar(v)) {
                return if self.sort_keys == FALSE {
                    self.live_object(ctx, id, depth)
                } else {
                    self.object_items(ctx, id, depth)
                };
            }
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
    fn snapshot(ctx: &mut Context, source: Id) -> Result<Id> {
        let iterator = ctx.iterator(source)?;
        let list = ctx.alloc(Value::List(Vec::new()));
        super::builtins::call(ctx, "list.extend", &[list, iterator], &[])?;
        Ok(list)
    }

    /// Read the mapping through its items method and sort the returned pairs when requested.
    fn object_items(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let method = ctx.lookup_attr(id, "items")?;
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        let items = if matches!(ctx.heap.get(result), Value::List(_)) {
            result
        } else {
            Self::snapshot(ctx, result)?
        };
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(items);
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

    /// Pull the next item only after its predecessor has reached the stream.
    fn stream_array(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let iter = ctx.iterator(id)?;
        write_json_fragment(&mut self.output.text, "[")?;
        let mut i = 0;
        loop {
            let roots = ctx.heap.temporary_mark();
            let Some(item) = ctx.next_item(iter)? else {
                break;
            };
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(item);
            self.separator(ctx, i != 0, depth + 1)?;
            self.value(ctx, item, depth + 1)
                .map_err(|e| self.noted(ctx, e, id, Location::Index(i)))?;
            ctx.heap.release_temporaries_since(roots);
            i += 1;
        }
        self.close(ctx, "]", i != 0, depth)
    }

    /// Stream mapping items as they arrive, materializing them only for a requested sort.
    fn stream_object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        write_json_fragment(&mut self.output.text, "{")?;
        self.emit(ctx)?;
        let sort_keys = ctx.truth(self.sort_keys)?;
        let method = ctx.lookup_attr(id, "items")?;
        let mut items = ctx.call(method, Vec::new(), Vec::new())?;
        if sort_keys {
            let sorted = ctx.builtin("sorted");
            items = ctx.call(sorted, super::super::copy_ids(&[items])?, Vec::new())?;
        }
        let iter = ctx.iterator(items)?;
        let roots = ctx.heap.temporary_mark();
        let mut nonempty = false;
        while let Some(pair) = ctx.next_item(iter)? {
            let values = ctx.unpack_items(pair, 2, None)?;
            // The previous loop variables stay alive while the iterator
            // produces and unpacks their replacements.
            ctx.heap.reserve_temporaries(2)?;
            ctx.heap.release_temporaries_since(roots);
            ctx.heap.keep(values[0]);
            ctx.heap.keep(values[1]);
            self.member(ctx, id, &mut nonempty, values[0], values[1], depth + 1)?;
        }
        self.close(ctx, "}", nonempty, depth)?;
        self.emit(ctx)
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
                let name = if self.stream.is_some() {
                    let class = ctx.lookup_attr(key, "__class__")?;
                    let name = ctx.lookup_attr(class, "__name__")?;
                    super::builtins::format_value(ctx, name, None)?
                } else {
                    // The one-shot encoder reports up to 100 bytes of the actual type name.
                    let class = ctx.exact_type(key)?;
                    let name = ctx.class_name(class);
                    ctx.text(name[..name.floor_char_boundary(100)].to_owned())
                };
                let prefix = ctx.text("keys must be str, int, float, bool or None, not ");
                let message = ctx.concat_strings(prefix, name)?;
                return Err(ctx.exception("TypeError", &[message]));
            }
        };
        self.separator(ctx, *nonempty, depth)?;
        *nonempty = true;
        let separator = self
            .stream
            .is_some()
            .then(|| std::mem::take(&mut self.output));
        let normalized = if let Some(text) = converted {
            if self.quote.is_some() {
                let mark = ctx.heap.temporary_mark();
                let string = match key {
                    // CPython reuses the constant names but renders numeric keys afresh.
                    NONE | TRUE | FALSE => ctx.heap.string_literal(&text)?,
                    _ => ctx.heap.fresh_string(Value::Str(text.to_string()))?,
                };
                self.string(ctx, string)?;
                ctx.heap.release_temporaries_since(mark);
            } else {
                write_json_string(&mut self.output.text, &text, self.ascii)?;
            }
            self.stream.is_some().then_some(text)
        } else {
            self.string(ctx, key)?;
            None
        };
        if let Some(separator) = separator {
            // A rejected key doesn't write the pending separator.
            let encoded = std::mem::replace(&mut self.output, separator);
            self.emit(ctx)?;
            self.output = encoded;
        }
        self.emit(ctx)?;
        self.key_separator.write(ctx, &mut self.output)?;
        self.emit(ctx)?;
        self.value(ctx, value, depth)
            .map_err(|e| self.noted(ctx, e, owner, Location::Key(key, normalized.as_deref())))
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
                let output = encode(&mut ctx, value, &[], None).unwrap();
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
