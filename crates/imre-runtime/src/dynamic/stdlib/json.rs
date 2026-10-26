use std::cmp::Ordering;
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
        match ctx.heap.get(id) {
            Value::None => self.output.push_str("null"),
            Value::Bool(value) => self.output.push_str(if *value { "true" } else { "false" }),
            Value::Int(value) => write!(self.output, "{value}").unwrap(),
            Value::Float(value) => self.output.push_str(&float_text(*value)),
            Value::Str(value) => write_json_string(&mut self.output, value, self.ascii)?,
            _ => {
                if self.active.len() >= 512 {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum JSON nesting exceeded",
                    ));
                }
                if !self.active.insert(id) {
                    return Err(Error::new("ValueError", "Circular reference detected"));
                }
                if self.default.is_some() {
                    ctx.heap.keep(id);
                }
                match ctx.heap.get(id) {
                    Value::List(_) | Value::Tuple(_) | Value::NamedTuple { .. } => {
                        self.output.push('[');
                        let mut i = 0;
                        while let Value::List(items)
                        | Value::Tuple(items)
                        | Value::NamedTuple { values: items, .. } = ctx.heap.get(id)
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
                        self.object(ctx, entries.to_vec())?;
                    }
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
            }
        }
        Ok(())
    }

    fn object(&mut self, ctx: &mut Context, mut entries: Vec<(Id, Id)>) -> Result<()> {
        if self.default.is_some() {
            // A callback can remove an entry while the encoder still holds its key and value.
            for &(key, value) in &entries {
                ctx.heap.keep(key);
                ctx.heap.keep(value);
            }
        }
        if self.sort_keys {
            let mut error = None;
            entries.sort_by(|(a, _), (b, _)| {
                ctx.compare(*a, *b)
                    .unwrap_or_else(|e| {
                        error.get_or_insert(e);
                        None
                    })
                    .unwrap_or(Ordering::Equal)
            });
            if let Some(error) = error {
                return Err(error);
            }
        }
        self.output.push('{');
        for (i, (key, value)) in entries.into_iter().enumerate() {
            if i != 0 {
                self.output.push_str(&self.item_separator);
            }
            match ctx.heap.get(key) {
                Value::Str(s) => write_json_string(&mut self.output, s, self.ascii)?,
                Value::Int(n) => write_json_string(&mut self.output, &n.to_string(), self.ascii)?,
                Value::Float(n) => {
                    write_json_string(&mut self.output, &float_text(*n), self.ascii)?
                }
                Value::None => self.output.push_str("\"null\""),
                Value::Bool(v) => self
                    .output
                    .push_str(if *v { "\"true\"" } else { "\"false\"" }),
                _ => return Err(Error::new("TypeError", "unsupported JSON object key")),
            }
            self.output.push_str(&self.key_separator);
            self.value(ctx, value)?;
        }
        self.output.push('}');
        Ok(())
    }
}

fn float_text(value: f64) -> Str {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        crate::floats::render(value)
    }
}
