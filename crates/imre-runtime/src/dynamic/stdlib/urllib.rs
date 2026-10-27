//! URL operations used by runtime-imported request handlers.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::builtins::bind;
use crate::objects::{UrlInput, urlquote, urlsplit};

mod quote;

const FIELDS: [&str; 5] = ["scheme", "netloc", "path", "query", "fragment"];
const SPLIT: &str = "SplitResult";
const SPLIT_BYTES: &str = "SplitResultBytes";

fn input(ctx: &Context, id: Id) -> Result<UrlInput<'_>> {
    match ctx.heap.get(id) {
        Value::Str(s) => Ok(UrlInput::Text(s)),
        Value::Bytes(b) => Ok(UrlInput::Bytes(b)),
        _ => Err(Error::new(
            "TypeError",
            format!("expected str or bytes, got '{}'", ctx.type_name(id)),
        )),
    }
}

fn required(id: Option<Id>, function: &str, name: &str) -> Result<Id> {
    id.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{function}() missing 1 required positional argument: '{name}'"),
        )
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "quote" | "quote_plus" | "quote_from_bytes" => quote::call(ctx, name, args, kwargs),
        "unquote" => {
            let [text, encoding, errors] =
                bind(name, args, kwargs, ["string", "encoding", "errors"])?;
            unquote(ctx, required(text, name, "string")?, encoding, errors)
        }
        "urlsplit" => {
            let [url, scheme, fragments] =
                bind(name, args, kwargs, ["url", "scheme", "allow_fragments"])?;
            split(ctx, required(url, name, "url")?, scheme, fragments)
        }
        "urlunsplit" | "SplitResult.geturl" => {
            let [parts] = bind(name, args, kwargs, ["components"])?;
            unsplit(ctx, required(parts, name, "components")?)
        }
        SPLIT | SPLIT_BYTES => {
            let arguments = bind(name, args, kwargs, FIELDS)?;
            let mut values = [NONE; 5];
            for (i, field) in FIELDS.iter().enumerate() {
                values[i] = required(arguments[i], name, field)?;
            }
            Ok(result(
                ctx,
                if name == SPLIT { SPLIT } else { SPLIT_BYTES },
                values,
            ))
        }
        _ => unreachable!("registered urllib.parse operation"),
    }
}

fn codec_option<'a>(
    ctx: &'a Context,
    value: Option<Id>,
    default: &'static str,
    name: &str,
) -> Result<Option<&'a str>> {
    match value {
        None => Ok(Some(default)),
        Some(NONE) => Ok(None),
        Some(id) => ctx.str_value(id).map(Some).map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "decode() argument '{name}' must be str, not {}",
                    ctx.type_name(id)
                ),
            )
        }),
    }
}

fn unquote(ctx: &mut Context, id: Id, encoding: Option<Id>, errors: Option<Id>) -> Result<Id> {
    let source = match input(ctx, id) {
        Ok(source) => source,
        Err(_) => {
            // CPython first checks for '%' in a non-bytes input and then its split attribute.
            let percent = ctx.text("%");
            let contains = ctx.contains(percent, id).map_err(|err| {
                if err.kind == "TypeError" {
                    Error::new(
                        "TypeError",
                        format!(
                            "argument of type '{}' is not a container or iterable",
                            ctx.type_name(id)
                        ),
                    )
                } else {
                    err
                }
            })?;
            if !contains {
                ctx.lookup_attr(id, "split").map_err(|err| {
                    if err.kind == "AttributeError" {
                        Error::new(
                            "AttributeError",
                            format!("'{}' object has no attribute 'split'", ctx.type_name(id)),
                        )
                    } else {
                        err
                    }
                })?;
                return Ok(id);
            }
            return Err(Error::new(
                "TypeError",
                format!(
                    "expected string or bytes-like object, got '{}'",
                    ctx.type_name(id)
                ),
            ));
        }
    };
    // CPython returns unescaped text before inspecting its codec options.
    if source.is_unescaped_text() {
        return Ok(id);
    }
    let text = urlquote::decode_component(
        source,
        codec_option(ctx, encoding, "utf-8", "encoding")?,
        codec_option(ctx, errors, "replace", "errors")?,
    )?;
    Ok(ctx.text(text))
}

fn result(ctx: &mut Context, class: &'static str, fields: [Id; 5]) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some(class),
        names: FIELDS.map(str::to_owned).into(),
        values: fields.into(),
        extras: Vec::new(),
    })
}

fn split(ctx: &mut Context, url: Id, scheme: Option<Id>, fragments: Option<Id>) -> Result<Id> {
    let fragments = ctx.optional_truth(fragments, true)?;
    let url = input(ctx, url)?;
    let scheme = scheme
        .map(|id| input(ctx, id))
        .transpose()?
        .unwrap_or(UrlInput::Text(""));
    let bytes = matches!(url, UrlInput::Bytes(_));
    let scheme_bytes = matches!(scheme, UrlInput::Bytes(_));
    if bytes != scheme_bytes && !scheme.as_bytes().is_empty() {
        return Err(Error::new(
            "TypeError",
            "Cannot mix str and non-str arguments",
        ));
    }
    let url = url.decoded()?;
    if !bytes && scheme_bytes {
        return Err(Error::new(
            "TypeError",
            "a bytes-like object is required, not 'str'",
        ));
    }
    let fields = urlsplit::with_split_fields(url, scheme.decoded()?, fragments, |fields| {
        Ok(fields.map(|s| {
            if bytes {
                Value::Bytes(s.as_bytes().to_vec())
            } else {
                Value::Str(s.to_owned())
            }
        }))
    })?;
    let ids = fields.map(|field| ctx.alloc(field));
    Ok(result(ctx, if bytes { SPLIT_BYTES } else { SPLIT }, ids))
}

pub(in crate::dynamic) fn property(
    ctx: &mut Context,
    netloc: Id,
    name: &str,
) -> Result<Option<Id>> {
    let index = match name {
        "username" => 0,
        "password" => 1,
        "hostname" => 2,
        "port" => 3,
        _ => return Ok(None),
    };
    let netloc = input(ctx, netloc)?;
    let value = match urlsplit::netloc_property(netloc, index)? {
        None => NONE,
        Some(urlsplit::NetlocValue::Port(n)) => ctx.int(i64::from(n)),
        Some(urlsplit::NetlocValue::Text(v)) => {
            let bytes = v.into_owned();
            if matches!(netloc, UrlInput::Bytes(_)) {
                ctx.alloc(Value::Bytes(bytes))
            } else {
                ctx.text(String::from_utf8(bytes).expect("text authority is UTF-8"))
            }
        }
    };
    Ok(Some(value))
}

fn unsplit(ctx: &mut Context, components: Id) -> Result<Id> {
    let items = ctx.items(components)?;
    if items.is_empty() {
        return Err(Error::new("IndexError", "tuple index out of range"));
    }
    if items.len() != 5 {
        let message = if items.len() > 5 {
            format!(
                "too many values to unpack (expected 6, got {})",
                items.len() + 1
            )
        } else {
            format!(
                "not enough values to unpack (expected 6, got {})",
                items.len() + 1
            )
        };
        return Err(Error::new("ValueError", message));
    }
    let bytes = matches!(input(ctx, items[0])?, UrlInput::Bytes(_));
    let mut fields = [""; 5];
    for (field, id) in fields.iter_mut().zip(items) {
        let value = input(ctx, id)?;
        if bytes != matches!(value, UrlInput::Bytes(_)) && !value.as_bytes().is_empty() {
            return Err(Error::new(
                "TypeError",
                "Cannot mix str and non-str arguments",
            ));
        }
        *field = value.decoded()?;
    }
    let out = urlsplit::join_fields(fields)?;
    Ok(if bytes {
        ctx.alloc(Value::Bytes(out.into_bytes()))
    } else {
        ctx.text(out)
    })
}
