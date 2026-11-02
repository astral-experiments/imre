use std::collections::{HashMap, HashSet};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::objects::{
    DecodedJson, JsonNumber, decode_json, decode_json_bytes, owned_bytes_input,
    write_json_fragment, write_json_string,
};
use crate::{Str, StrOwner};

pub(super) fn loads(ctx: &mut Context, input: Id) -> Result<Id> {
    match ctx.heap.get(input) {
        Value::Str(text) => {
            let text = Str::try_new(text).map_err(|_| crate::Error::allocation())?;
            if text.starts_with('\u{feff}') {
                let document = StrOwner::try_new(text).map_err(|_| crate::Error::allocation())?;
                return Err(crate::objects::json_bom_error(document).into());
            }
            parse(ctx, &text)
        }
        Value::Bytes(bytes) => {
            let owner = owned_bytes_input(bytes)?;
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
    allow_nan: Id,
    indent: Option<String>,
    sort_keys: Id,
    skipkeys: Id,
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
        allow_nan,
        indent,
        sort_keys,
        skipkeys,
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
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.value_inner(ctx, id, depth)
        })
    }

    fn value_inner(&mut self, ctx: &mut Context, id: Id, depth: usize) -> Result<()> {
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
                            self.separator(i != 0, depth + 1)?;
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
    fn separator(&mut self, after_first: bool, depth: usize) -> Result<()> {
        if after_first {
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
        if ctx.truth(self.sort_keys)? {
            super::builtins::sort_values(ctx, &mut entries, false, |(key, _)| key)?;
        }
        write_json_fragment(&mut self.output, "{")?;
        let mut nonempty = false;
        for (key, value) in entries {
            self.member(ctx, &mut nonempty, key, value, depth + 1)?;
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
        if ctx.truth(self.sort_keys)? {
            super::builtins::call(ctx, "list.sort", &[items], &[])?;
        }
        write_json_fragment(&mut self.output, "{")?;
        let mut i = 0;
        let mut nonempty = false;
        while let Value::List(pairs) = ctx.heap.get(items) {
            let Some(&pair) = pairs.get(i) else { break };
            let values = ctx.tuple_items(pair)?;
            let Some(&[key, value]) = values.as_deref() else {
                return Err(Error::new("ValueError", "items must return 2-tuples"));
            };
            self.member(ctx, &mut nonempty, key, value, depth + 1)?;
            i += 1;
        }
        self.close(ctx, "}", nonempty, depth)
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
            self.separator(i != 0, depth + 1)?;
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
            self.member(ctx, &mut nonempty, values[0], values[1], depth + 1)?;
        }
        self.close(ctx, "}", nonempty, depth)?;
        self.emit(ctx)
    }

    /// Write an object member after any sorting and pair validation.
    fn member(
        &mut self,
        ctx: &mut Context,
        nonempty: &mut bool,
        key: Id,
        value: Id,
        depth: usize,
    ) -> Result<()> {
        let number;
        let text: &str = match ctx.heap.get(ctx.scalar_value(key)) {
            Value::Str(text) => text,
            Value::Int(n) => {
                number = crate::integer_text::decimal_string(n)?;
                &number
            }
            Value::Float(n) => {
                number = float_text(ctx, key, *n, self.allow_nan)?;
                &number
            }
            Value::None => "null",
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            Value::SurrogateStr(_) => {
                return Err(Error::new("TypeError", "unsupported JSON object key"));
            }
            _ => {
                if ctx.truth(self.skipkeys)? {
                    return Ok(());
                }
                return Err(Error::new("TypeError", "unsupported JSON object key"));
            }
        };
        self.separator(*nonempty, depth)?;
        *nonempty = true;
        let separator = self
            .stream
            .is_some()
            .then(|| std::mem::take(&mut self.output));
        write_json_string(&mut self.output, text, self.ascii)?;
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
