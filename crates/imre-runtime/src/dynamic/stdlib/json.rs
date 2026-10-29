use std::collections::{HashMap, HashSet};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::objects::{
    DecodedJson, JsonNumber, decode_json, decode_json_bytes, write_json_fragment, write_json_string,
};
use crate::{Str, StrOwner};

pub(super) fn loads(ctx: &mut Context, input: Id) -> Result<Id> {
    match ctx.heap.get(input) {
        Value::Str(text) => {
            let text = text.clone();
            if text.starts_with('\u{feff}') {
                return Err(crate::objects::json_bom_error(StrOwner::new(Str::from(text))).into());
            }
            parse(ctx, &text)
        }
        Value::Bytes(bytes) => {
            let owner = crate::BytesOwner::new(bytes.clone());
            parse(ctx, &decode_json_bytes(&owner)?)
        }
        _ => Err(Error::new(
            "TypeError",
            format!(
                "the JSON object must be str, bytes or bytearray, not {}",
                ctx.type_name(input)
            ),
        )),
    }
}

fn parse(ctx: &mut Context, text: &str) -> Result<Id> {
    let mut keys = HashMap::new();
    Ok(decode_json(
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
    )?)
}

struct Encoder {
    ascii: bool,
    allow_nan: bool,
    indent: Option<String>,
    sort_keys: bool,
    item_separator: String,
    key_separator: String,
    default: Option<Id>,
    stream: Option<Id>,
    active: HashSet<Id>,
    output: String,
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
    let indent = match option("indent") {
        None | Some(NONE) => None,
        Some(_) if stream.is_none() && ctx.str_value(id).is_ok() => None,
        Some(unit) => {
            let unit = if ctx.str_value(unit).is_ok() {
                unit
            } else {
                let space = ctx.text(" ");
                ctx.binary("*", space, unit)?
            };
            Some(ctx.str_value(unit)?.to_owned())
        }
    };
    let (item_separator, key_separator) = match option("separators") {
        None | Some(NONE) => (
            (if indent.is_some() { "," } else { ", " }).to_owned(),
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
    let mut encoder = Encoder {
        ascii: ctx.optional_truth(option("ensure_ascii"), true)?,
        allow_nan: ctx.optional_truth(option("allow_nan"), true)?,
        indent,
        sort_keys: ctx.optional_truth(option("sort_keys"), false)?,
        item_separator,
        key_separator,
        default: option("default").filter(|v| *v != NONE),
        stream,
        active: HashSet::new(),
        output: String::new(),
    };
    encoder.value(ctx, id, 0)?;
    Ok(if stream.is_some() {
        NONE
    } else {
        ctx.text(encoder.output)
    })
}

impl Encoder {
    /// Deliver each completed fragment before running later user code.
    fn emit(&mut self, ctx: &mut Context) -> Result<()> {
        if let Some(stream) = self.stream
            && !self.output.is_empty()
        {
            let roots = ctx.heap.temporary_mark();
            let chunk = ctx.text(std::mem::take(&mut self.output));
            let write = ctx.lookup_attr(stream, "write")?;
            ctx.call(write, super::super::copy_ids(&[chunk])?, Vec::new())?;
            ctx.heap.release_temporaries_since(roots);
        }
        Ok(())
    }

    fn value(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        match ctx.heap.get(ctx.scalar_value(id)) {
            Value::None => write_json_fragment(&mut self.output, "null")?,
            Value::Bool(value) => {
                write_json_fragment(&mut self.output, if *value { "true" } else { "false" })?
            }
            Value::Int(value) => write_json_fragment(
                &mut self.output,
                &crate::integer_text::decimal_string(value)?,
            )?,
            Value::Float(value) => write_json_fragment(
                &mut self.output,
                &float_text(ctx, id, *value, self.allow_nan)?,
            )?,
            Value::Str(value) => write_json_string(&mut self.output, value, self.ascii)?,
            _ => {
                if self.active.len() >= 512 {
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
                    write_json_fragment(&mut self.output, empty)?;
                    ctx.heap.release_temporaries_since(roots);
                    return self.emit(ctx);
                }
                if !self.active.insert(id) {
                    return Err(Error::new("ValueError", "Circular reference detected"));
                }
                match ctx.heap.get(contents) {
                    _ if streaming && sequence => self.stream_array(ctx, id, depth)?,
                    _ if streaming => self.stream_object(ctx, id, depth)?,
                    Value::List(_) | Value::Tuple(_) | Value::NamedTuple { .. } => {
                        write_json_fragment(&mut self.output, "[")?;
                        let mut i = 0;
                        while let Value::List(items)
                        | Value::Tuple(items)
                        | Value::NamedTuple { values: items, .. } = ctx.heap.get(contents)
                        {
                            let Some(&item) = items.get(i) else { break };
                            self.separator(i, depth + 1)?;
                            self.value(ctx, item, depth + 1)?;
                            i += 1;
                        }
                        self.close(ctx, "]", i != 0, depth)?;
                    }
                    Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
                        self.object(ctx, super::super::copy_ids(entries)?, depth)?;
                    }
                    Value::Instance {
                        dictionary: Some(_),
                        ..
                    } => self.subclass_object(ctx, id, depth)?,
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
                        self.value(ctx, result, depth)?;
                    }
                }
                self.active.remove(&id);
                ctx.heap.release_temporaries_since(roots);
            }
        }
        self.emit(ctx)
    }

    /// Write a separator and the indentation before a sequence item or object member.
    fn separator(&mut self, i: usize, depth: usize) -> Result<()> {
        if i != 0 {
            write_json_fragment(&mut self.output, &self.item_separator)?;
        }
        self.newline(depth)
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
            self.newline(depth)?;
            self.emit(ctx)?;
        }
        write_json_fragment(&mut self.output, delimiter)?;
        Ok(())
    }

    fn newline(&mut self, depth: usize) -> Result<()> {
        if let Some(indent) = &self.indent {
            crate::objects::JsonIndent::Text(indent).write_newline(&mut self.output, depth)?;
        }
        Ok(())
    }

    fn object(
        &mut self,
        ctx: &mut Context,
        mut entries: Vec<(Id, Id)>,
        depth: usize,
    ) -> Result<()> {
        // Encoding a nested subclass can call Python and clear the original mapping.
        ctx.heap
            .reserve_temporaries(entries.len().saturating_mul(2))?;
        for &(key, value) in &entries {
            ctx.heap.keep(key);
            ctx.heap.keep(value);
        }
        if self.sort_keys {
            super::builtins::sort_values(ctx, &mut entries, false, |(key, _)| key)?;
        }
        write_json_fragment(&mut self.output, "{")?;
        let nonempty = !entries.is_empty();
        for (i, (key, value)) in entries.into_iter().enumerate() {
            self.member(ctx, i, key, value, depth + 1)?;
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

    /// Read a dict subclass through its items method and sort the returned pairs when requested.
    fn subclass_object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let method = ctx.lookup_attr(id, "items")?;
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        let items = if matches!(ctx.heap.get(result), Value::List(_)) {
            result
        } else {
            Self::snapshot(ctx, result)?
        };
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(items);
        if self.sort_keys {
            super::builtins::call(ctx, "list.sort", &[items], &[])?;
        }
        write_json_fragment(&mut self.output, "{")?;
        let mut i = 0;
        while let Value::List(pairs) = ctx.heap.get(items) {
            let Some(&pair) = pairs.get(i) else { break };
            let values = ctx.tuple_items(pair)?;
            let Some(&[key, value]) = values.as_deref() else {
                return Err(Error::new("ValueError", "items must return 2-tuples"));
            };
            self.member(ctx, i, key, value, depth + 1)?;
            i += 1;
        }
        self.close(ctx, "}", i != 0, depth)
    }

    /// Pull the next item only after its predecessor has reached the stream.
    fn stream_array(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        let iter = ctx.iterator(id)?;
        write_json_fragment(&mut self.output, "[")?;
        let mut i = 0;
        loop {
            let roots = ctx.heap.temporary_mark();
            let Some(item) = ctx.next_item(iter)? else {
                break;
            };
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(item);
            self.separator(i, depth + 1)?;
            self.value(ctx, item, depth + 1)?;
            ctx.heap.release_temporaries_since(roots);
            i += 1;
        }
        self.close(ctx, "]", i != 0, depth)
    }

    /// Stream mapping items as they arrive, materializing them only for a requested sort.
    fn stream_object(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
        write_json_fragment(&mut self.output, "{")?;
        self.emit(ctx)?;
        let method = ctx.lookup_attr(id, "items")?;
        let mut items = ctx.call(method, Vec::new(), Vec::new())?;
        if self.sort_keys {
            let sorted = ctx.builtin("sorted");
            items = ctx.call(sorted, super::super::copy_ids(&[items])?, Vec::new())?;
        }
        let iter = ctx.iterator(items)?;
        let mut i = 0;
        loop {
            let roots = ctx.heap.temporary_mark();
            let Some(pair) = ctx.next_item(iter)? else {
                break;
            };
            let values = ctx.unpack_items(pair, 2, None)?;
            ctx.heap.reserve_temporaries(2)?;
            ctx.heap.keep(values[0]);
            ctx.heap.keep(values[1]);
            self.member(ctx, i, values[0], values[1], depth + 1)?;
            ctx.heap.release_temporaries_since(roots);
            i += 1;
        }
        self.close(ctx, "}", i != 0, depth)
    }

    /// Write an object member after any sorting and pair validation.
    fn member(
        &mut self,
        ctx: &mut Context,
        i: usize,
        key: Id,
        value: Id,
        depth: usize,
    ) -> Result<()> {
        self.separator(i, depth)?;
        let separator = self
            .stream
            .is_some()
            .then(|| std::mem::take(&mut self.output));
        match ctx.heap.get(ctx.scalar_value(key)) {
            Value::Str(s) => write_json_string(&mut self.output, s, self.ascii)?,
            Value::Int(n) => write_json_string(
                &mut self.output,
                &crate::integer_text::decimal_string(n)?,
                self.ascii,
            )?,
            Value::Float(n) => {
                let text = float_text(ctx, key, *n, self.allow_nan)?;
                write_json_string(&mut self.output, &text, self.ascii)?
            }
            Value::None => write_json_fragment(&mut self.output, "\"null\"")?,
            Value::Bool(v) => {
                write_json_fragment(&mut self.output, if *v { "\"true\"" } else { "\"false\"" })?
            }
            _ => return Err(Error::new("TypeError", "unsupported JSON object key")),
        }
        if let Some(separator) = separator {
            // A rejected key doesn't write the pending separator.
            let encoded = std::mem::replace(&mut self.output, separator);
            self.emit(ctx)?;
            self.output = encoded;
        }
        self.emit(ctx)?;
        write_json_fragment(&mut self.output, &self.key_separator)?;
        self.emit(ctx)?;
        self.value(ctx, value, depth)
    }
}

/// Spell nonfinite values as JSON extensions unless strict encoding rejects them.
fn float_text(ctx: &mut Context, id: Id, value: f64, allow_nan: bool) -> Result<Str> {
    if value.is_finite() {
        return Ok(crate::floats::render(value));
    }
    if !allow_nan {
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
