//! URL operations used by runtime-imported request handlers.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::builtins::{bind, boolean};
use crate::dynamic::copy_ids;
use crate::objects::{UrlInput, urlquote, urlsplit};

mod join;
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
    for name in [
        "urlsplit",
        "urlparse",
        "urljoin",
        "urlunsplit",
        "urlunparse",
        "unquote",
        "urlencode",
    ] {
        let operation = ctx.builtin(&format!("urllib.parse.{name}"));
        ctx.bind(scope, &format!("_imre_{name}"), operation);
    }
    for name in ["_check_netloc", "_netloc_property"] {
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
        "urlunsplit" => unsplit::<5>(ctx, args, kwargs),
        "urlunparse" => unsplit::<6>(ctx, args, kwargs),
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

/// Check the evaluated fields without invoking truth or decoding protocols.
fn plain_components<const N: usize>(ctx: &Context, components: &[Id]) -> Option<([Id; N], bool)> {
    let items: [Id; N] = components.try_into().ok()?;
    let bytes = match ctx.heap.get(items[0]) {
        Value::Str(_) => false,
        Value::Bytes(_) => true,
        _ => return None,
    };
    // In text mode a falsey path keeps its value for slicing, concatenation
    // or %s formatting with params. Only optional fields can be omitted.
    if !bytes && items[2] == NONE {
        return None;
    }
    let plain = items.iter().all(|&id| match ctx.heap.get(id) {
        Value::Str(_) => !bytes,
        Value::Bytes(value) => bytes && value.is_ascii(),
        _ => id == NONE,
    });
    plain.then_some((items, bytes))
}

fn component_text(ctx: &Context, id: Id) -> &str {
    match ctx.heap.get(id) {
        Value::Str(s) => s,
        Value::Bytes(b) => std::str::from_utf8(b).expect("validated ASCII URL component"),
        _ if id == NONE => "",
        _ => unreachable!("validated plain URL component"),
    }
}

fn unsplit<const N: usize>(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [components, fallback] = args else {
        return Err(Error::new(
            "TypeError",
            "expected URL components and their coercion function",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "expected positional URL components",
        ));
    }
    // Tuple subclasses such as ParseResult inherit immutable storage. Consult
    // the live type slot because a subclass can replace __iter__.
    let sequence = match ctx.heap.get(*components) {
        Value::Tuple(_) | Value::List(_) => *components,
        _ if let Some((storage, _)) = ctx.sequence_payload(*components)
            && matches!(ctx.heap.get(storage), Value::Tuple(_))
            && ctx.slot(*components, "__iter__").is_none_or(
                |slot| matches!(ctx.heap.get(slot), Value::Native(name) if name == "tuple.__iter__"),
            ) =>
        {
            storage
        }
        _ => ctx.collect_iterable(*components, false)?,
    };
    let (Value::Tuple(fields) | Value::List(fields)) = ctx.heap.get(sequence) else {
        unreachable!("expanded URL fields")
    };
    let Some((items, bytes)) = plain_components::<N>(ctx, fields) else {
        // Keep a stable view across the call into Python. Immutable tuple
        // storage is already a snapshot, including for standard URL results.
        let snapshot = match ctx.heap.get(sequence) {
            Value::List(items) => ctx.alloc(Value::Tuple(copy_ids(items)?)),
            _ => sequence,
        };
        return ctx.call(*fallback, vec![snapshot, boolean(N == 6)], Vec::new());
    };
    let scheme = component_text(ctx, items[0]);
    let registered = if !scheme.is_empty() && component_text(ctx, items[1]).is_empty() {
        // Membership may run Python that clears the original list and collects.
        ctx.heap.reserve_temporaries(N)?;
        for id in items {
            ctx.heap.keep(id);
        }
        let scheme = if bytes {
            ctx.text(component_text(ctx, items[0]).to_owned())
        } else {
            items[0]
        };
        let module = ctx.module("urllib.parse")?;
        let schemes = ctx.lookup_attr(module, "uses_netloc")?;
        ctx.contains(scheme, schemes)?
    } else {
        false
    };
    let fields = items.map(|id| component_text(ctx, id));
    let out = match *fields.as_slice() {
        [s, n, p, q, f] => urlsplit::join_fields([s, n, p, q, f], registered)?,
        [s, n, p, params, q, f] => {
            urlsplit::join_parse_fields([s, n, p, params, q, f], registered)?
        }
        _ => unreachable!("URL result arity"),
    };
    if !bytes && out == fields[2] {
        return Ok(items[2]);
    }
    Ok(if bytes {
        ctx.alloc(Value::Bytes(out.into_bytes()))
    } else {
        ctx.text(out)
    })
}
