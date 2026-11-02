//! URL operations used by runtime-imported request handlers.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::builtins::bind;
use crate::objects::{UrlInput, urlquote, urlsplit};

mod join;
mod quote;
mod urlencode;

/// Bind the mutable scheme registry once, in this module's Python namespace.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for (name, schemes) in [
        ("uses_relative", urlsplit::join::USES_RELATIVE),
        ("uses_netloc", urlsplit::USES_NETLOC),
        (
            "uses_params",
            &[
                "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps",
                "rtspu", "sip", "sips", "mms", "sftp", "tel",
            ][..],
        ),
    ] {
        let items = schemes.iter().map(|&scheme| ctx.text(scheme)).collect();
        let registry = ctx.alloc(Value::List(items));
        ctx.bind(scope, name, registry);
    }
    for name in ["urlsplit", "urlparse"] {
        let operation = ctx.builtin(&format!("urllib.parse.{name}"));
        ctx.bind(scope, &format!("_imre_{name}"), operation);
    }
    for name in [
        "_check_netloc",
        "_netloc_property",
        "urljoin",
        "urlunparse",
        "urlunsplit",
    ] {
        let operation = ctx.builtin(&format!("urllib.parse.{name}"));
        ctx.bind(scope, name, operation);
    }
}

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
        "urlencode" => urlencode::call(ctx, args, kwargs),
        "urljoin" => {
            let [base, url, fragments] =
                bind(name, args, kwargs, ["base", "url", "allow_fragments"])?;
            join::join(
                ctx,
                required(base, name, "base")?,
                required(url, name, "url")?,
                fragments,
            )
        }
        "unquote" => {
            let [text, encoding, errors] =
                bind(name, args, kwargs, ["string", "encoding", "errors"])?;
            unquote(ctx, required(text, name, "string")?, encoding, errors)
        }
        "_check_netloc" => {
            let [netloc] = bind(name, args, kwargs, ["netloc"])?;
            urlsplit::check_netloc(ctx.str_value(required(netloc, name, "netloc")?)?)?;
            Ok(NONE)
        }
        "urlsplit" | "urlparse" => {
            let [url, scheme, fragments] =
                bind(name, args, kwargs, ["url", "scheme", "allow_fragments"])?;
            split(
                ctx,
                required(url, name, "url")?,
                scheme,
                fragments,
                name == "urlparse",
            )
        }
        "urlunsplit" => {
            let [parts] = bind(name, args, kwargs, ["components"])?;
            unsplit::<5>(ctx, required(parts, name, "components")?)
        }
        "urlunparse" => {
            let [parts] = bind(name, args, kwargs, ["components"])?;
            unsplit::<6>(ctx, required(parts, name, "components")?)
        }
        "_netloc_property" => {
            let [netloc, field] = bind(name, args, kwargs, ["netloc", "field"])?;
            let field = required(field, name, "field")?;
            let index = match ctx.str_value(field)? {
                "username" => 0,
                "password" => 1,
                "hostname" => 2,
                "port" => 3,
                other => {
                    return Err(Error::new(
                        "ValueError",
                        format!("unknown authority field: {other}"),
                    ));
                }
            };
            property(ctx, required(netloc, name, "netloc")?, index)
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
    let encoding = codec_option(ctx, encoding, "utf-8", "encoding")?;
    let errors = codec_option(ctx, errors, "replace", "errors")?;
    let text = if errors.is_some_and(|name| super::codecs::uses_python_handler(ctx, name)) {
        // A Python callback can allocate and move the boxed heap.
        let options = (encoding.map(str::to_owned), errors.map(str::to_owned));
        let owned = ctx.heap.get(id).clone();
        let input = match &owned {
            Value::Str(s) => UrlInput::Text(s),
            Value::Bytes(b) => UrlInput::Bytes(b),
            _ => unreachable!(),
        };
        urlquote::decode_component_with(
            input,
            options.0.as_deref(),
            options.1.as_deref(),
            |codec, data, policy| super::codecs::decode_slice(ctx, codec, data, policy),
        )?
    } else {
        urlquote::decode_component(source, encoding, errors).map_err(|e| ctx.decode_error(e))?
    };
    Ok(ctx.text(text))
}

fn split(
    ctx: &mut Context,
    url: Id,
    scheme: Option<Id>,
    fragments: Option<Id>,
    parse: bool,
) -> Result<Id> {
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
        Ok(fields.map(str::to_owned))
    })?;
    let [scheme, netloc, mut path, query, fragment] = fields;
    let fields = if parse {
        let module = ctx.module("urllib.parse")?;
        let registry = ctx.lookup_attr(module, "uses_params")?;
        let scheme_id = ctx.text(scheme.clone());
        let mut params = String::new();
        if ctx.contains(scheme_id, registry)? {
            let last_segment = path.rfind('/').unwrap_or(0);
            if let Some(offset) = path[last_segment..].find(';') {
                params = path.split_off(last_segment + offset + 1);
                path.pop();
            }
        }
        vec![scheme, netloc, path, params, query, fragment]
    } else {
        vec![scheme, netloc, path, query, fragment]
    };
    let fields = fields.into_iter().map(|field| ctx.text(field)).collect();
    Ok(ctx.alloc(Value::Tuple(fields)))
}

fn property(ctx: &mut Context, netloc: Id, index: u32) -> Result<Id> {
    let netloc = input(ctx, netloc)?;
    Ok(match urlsplit::netloc_property(netloc, index)? {
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
    })
}

fn unsplit<const N: usize>(ctx: &mut Context, components: Id) -> Result<Id> {
    let items = ctx.items(components)?;
    if items.is_empty() {
        return Err(Error::new("IndexError", "tuple index out of range"));
    }
    if items.len() != N {
        let message = if items.len() > N {
            format!(
                "too many values to unpack (expected {}, got {})",
                N + 1,
                items.len() + 1
            )
        } else {
            format!(
                "not enough values to unpack (expected {}, got {})",
                N + 1,
                items.len() + 1
            )
        };
        return Err(Error::new("ValueError", message));
    }
    let bytes = matches!(input(ctx, items[0])?, UrlInput::Bytes(_));
    let mut text = [NONE; N];
    for (field, id) in text.iter_mut().zip(items) {
        let value = input(ctx, id)?;
        if bytes != matches!(value, UrlInput::Bytes(_)) && !value.as_bytes().is_empty() {
            return Err(Error::new(
                "TypeError",
                "Cannot mix str and non-str arguments",
            ));
        }
        *field = match value {
            UrlInput::Text(_) => id,
            UrlInput::Bytes(_) => {
                let decoded = value.decoded()?.to_owned();
                ctx.text(decoded)
            }
        };
    }
    let registered = if !ctx.str_value(text[0])?.is_empty() && ctx.str_value(text[1])?.is_empty() {
        let module = ctx.module("urllib.parse")?;
        let schemes = ctx.lookup_attr(module, "uses_netloc")?;
        ctx.contains(text[0], schemes)?
    } else {
        false
    };
    let fields = text.map(|id| ctx.str_value(id).expect("coerced URL component"));
    let out = match *fields.as_slice() {
        [s, n, p, q, f] => urlsplit::join_fields([s, n, p, q, f], registered)?,
        [s, n, p, params, q, f] => {
            urlsplit::join_parse_fields([s, n, p, params, q, f], registered)?
        }
        _ => unreachable!("URL result arity"),
    };
    Ok(if bytes {
        ctx.alloc(Value::Bytes(out.into_bytes()))
    } else {
        ctx.text(out)
    })
}
