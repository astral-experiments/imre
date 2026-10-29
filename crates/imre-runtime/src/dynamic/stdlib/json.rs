use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::objects::{DecodedJson, JsonNumber, decode_json, decode_json_bytes, write_json_string};
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
    sort_keys: bool,
    item_separator: String,
    key_separator: String,
    default: Option<Id>,
    active: HashSet<Id>,
    output: String,
}

pub(super) fn dumps(ctx: &mut Context, id: Id, kwargs: &[(String, Id)]) -> Result<String> {
    let option = |name| {
        kwargs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| *value)
    };
    let (item_separator, key_separator) = match option("separators") {
        None | Some(NONE) => (", ".to_owned(), ": ".to_owned()),
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
        sort_keys: ctx.optional_truth(option("sort_keys"), false)?,
        item_separator,
        key_separator,
        default: option("default").filter(|v| *v != NONE),
        active: HashSet::new(),
        output: String::new(),
    };
    encoder.value(ctx, id)?;
    Ok(encoder.output)
}

impl Encoder {
    fn value(&mut self, ctx: &mut Context, id: Id) -> Result<()> {
        match ctx.heap.get(ctx.scalar_value(id)) {
            Value::None => self.output.push_str("null"),
            Value::Bool(value) => self.output.push_str(if *value { "true" } else { "false" }),
            Value::Int(value) => write!(self.output, "{value}").unwrap(),
            Value::Float(value) => {
                self.output
                    .push_str(&float_text(ctx, id, *value, self.allow_nan)?)
            }
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
                let contents = if matches!(ctx.heap.get(id), Value::Instance { .. })
                    && (ctx.builtin_slot_accepts(id, "list")
                        || ctx.builtin_slot_accepts(id, "tuple"))
                {
                    Self::snapshot(ctx, id)?
                } else {
                    id
                };
                // Materialize subclass iteration before detecting a repeated container. A
                // second call may return an empty sequence and end the recursion itself.
                let empty = match ctx.heap.get(contents) {
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
                    self.output.push_str(empty);
                    ctx.heap.release_temporaries_since(roots);
                    return Ok(());
                }
                if !self.active.insert(id) {
                    return Err(Error::new("ValueError", "Circular reference detected"));
                }
                match ctx.heap.get(contents) {
                    Value::List(_) | Value::Tuple(_) | Value::NamedTuple { .. } => {
                        self.output.push('[');
                        let mut i = 0;
                        while let Value::List(items)
                        | Value::Tuple(items)
                        | Value::NamedTuple { values: items, .. } = ctx.heap.get(contents)
                        {
                            let Some(&item) = items.get(i) else { break };
                            if i != 0 {
                                self.output.push_str(&self.item_separator);
                            }
                            self.value(ctx, item)?;
                            i += 1;
                        }
                        self.output.push(']');
                    }
                    Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
                        self.object(ctx, super::super::copy_ids(entries)?)?;
                    }
                    Value::Instance {
                        dictionary: Some(_),
                        ..
                    } => self.subclass_object(ctx, id)?,
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
                        let result = ctx.call(default, vec![id], Vec::new())?;
                        self.value(ctx, result)?;
                    }
                }
                self.active.remove(&id);
                ctx.heap.release_temporaries_since(roots);
            }
        }
        Ok(())
    }

    fn object(&mut self, ctx: &mut Context, mut entries: Vec<(Id, Id)>) -> Result<()> {
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
        self.output.push('{');
        for (i, (key, value)) in entries.into_iter().enumerate() {
            self.member(ctx, i, key, value)?;
        }
        self.output.push('}');
        Ok(())
    }

    /// Materialize a subclass iterator while keeping previously yielded values alive.
    fn snapshot(ctx: &mut Context, source: Id) -> Result<Id> {
        let iterator = ctx.iterator(source)?;
        let list = ctx.alloc(Value::List(Vec::new()));
        super::builtins::call(ctx, "list.extend", &[list, iterator], &[])?;
        Ok(list)
    }

    /// Read a dict subclass through its items method and sort the returned pairs when requested.
    fn subclass_object(&mut self, ctx: &mut Context, id: Id) -> Result<()> {
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
        self.output.push('{');
        let mut i = 0;
        while let Value::List(pairs) = ctx.heap.get(items) {
            let Some(&pair) = pairs.get(i) else { break };
            let values = ctx.tuple_items(pair)?;
            let Some(&[key, value]) = values.as_deref() else {
                return Err(Error::new("ValueError", "items must return 2-tuples"));
            };
            self.member(ctx, i, key, value)?;
            i += 1;
        }
        self.output.push('}');
        Ok(())
    }

    /// Write an object member after any sorting and pair validation.
    fn member(&mut self, ctx: &mut Context, i: usize, key: Id, value: Id) -> Result<()> {
        if i != 0 {
            self.output.push_str(&self.item_separator);
        }
        match ctx.heap.get(ctx.scalar_value(key)) {
            Value::Str(s) => write_json_string(&mut self.output, s, self.ascii)?,
            Value::Int(n) => write_json_string(&mut self.output, &n.to_string(), self.ascii)?,
            Value::Float(n) => {
                let text = float_text(ctx, key, *n, self.allow_nan)?;
                write_json_string(&mut self.output, &text, self.ascii)?
            }
            Value::None => self.output.push_str("\"null\""),
            Value::Bool(v) => self
                .output
                .push_str(if *v { "\"true\"" } else { "\"false\"" }),
            _ => return Err(Error::new("TypeError", "unsupported JSON object key")),
        }
        self.output.push_str(&self.key_separator);
        self.value(ctx, value)
    }
}

/// Spell nonfinite values as JSON extensions unless strict encoding rejects them.
fn float_text(ctx: &mut Context, id: Id, value: f64, allow_nan: bool) -> Result<Str> {
    if value.is_finite() {
        return Ok(crate::floats::render(value));
    }
    if !allow_nan {
        let repr = ctx.builtin("repr");
        let text = ctx.call(repr, vec![id], Vec::new())?;
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
