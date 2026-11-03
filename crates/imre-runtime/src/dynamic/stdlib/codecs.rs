//! Box the entries and stateless decoders of the native codec registry.
use std::collections::HashMap;

mod bytes_escape;
mod encode_error;
mod raw_unicode_escape;
mod surrogate_utf8;
mod utf16;
mod wide_unicode;
pub(super) use wide_unicode::decode_chunk as decode_wide_chunk;
pub(super) use wide_unicode::{encode as wide_encode, extended as wide_extended};

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{Codec, CodecInfo, DecodeIssue, FixedUnicode, Object, StrEncoder, Utf16Order};

#[derive(Default)]
pub(in crate::dynamic) struct Registry {
    entries: HashMap<*const Object, (CodecInfo, Id)>,
    initialized: bool,
    search_path: Vec<Id>,
    // None denotes an existing stateless native transform with no boxed CodecInfo.
    search_cache: indexmap::IndexMap<String, Option<Id>>,
    handlers: HashMap<String, Id>,
    // Built-in and module-level codec functions are shared across alias families.
    functions: HashMap<&'static str, (Id, Id)>,
}

impl Registry {
    pub(in crate::dynamic) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.entries
            .values()
            .map(|(_, entry)| entry)
            .chain(
                self.functions
                    .values()
                    .flat_map(|(encode, decode)| [encode, decode]),
            )
            .chain(self.handlers.values())
            .chain(&self.search_path)
            .chain(self.search_cache.values().filter_map(Option::as_ref))
    }
}

fn type_error(message: impl Into<String>) -> Error {
    Error::new("TypeError", message)
}

/// Built-in handler objects are shared between the module and the per-interpreter registry.
fn handler_builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        "strict" => "codecs.strict_errors",
        "ignore" => "codecs.ignore_errors",
        "replace" => "codecs.replace_errors",
        "xmlcharrefreplace" => "codecs.xmlcharrefreplace_errors",
        "backslashreplace" => "codecs.backslashreplace_errors",
        "namereplace" => "codecs.namereplace_errors",
        "surrogateescape" => "codecs.surrogateescape",
        "surrogatepass" => "codecs.surrogatepass",
        _ => return None,
    })
}

fn handler(ctx: &mut Context, name: &str) -> Result<Id> {
    if let Some(&id) = ctx.codec_entries.handlers.get(name) {
        return Ok(id);
    }
    if let Some(builtin) = handler_builtin(name) {
        return Ok(ctx.builtin(builtin));
    }
    Err(crate::objects::unknown_handler(name).into())
}

pub(super) fn error_registry(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    let (name, callback) = match (function, args) {
        ("lookup_error", [name]) => (*name, None),
        ("register_error", [name, callback]) => (*name, Some(*callback)),
        ("lookup_error", _) => {
            return Err(type_error(format!(
                "_codecs.lookup_error() takes exactly one argument ({} given)",
                args.len()
            )));
        }
        _ => {
            return Err(type_error(format!(
                "register_error expected 2 arguments, got {}",
                args.len()
            )));
        }
    };
    let Value::Str(name_str) = ctx.heap.get(ctx.scalar_value(name)) else {
        let kind = if name == NONE {
            "None"
        } else {
            ctx.type_name(name)
        };
        let arg = if callback.is_some() {
            "argument 1"
        } else {
            "argument"
        };
        return Err(type_error(format!(
            "{function}() {arg} must be str, not {kind}"
        )));
    };
    if name_str.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    let name = name_str.clone();
    if let Some(callback) = callback {
        if !ctx.callable(callback) {
            return Err(type_error("handler must be callable"));
        }
        ctx.codec_entries
            .handlers
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        ctx.codec_entries.handlers.insert(name, callback);
        Ok(NONE)
    } else {
        handler(ctx, &name)
    }
}

/// Invoke a built-in handler retrieved by lookup_error, including under a registered alias.
pub(super) fn builtin_handler(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{function}() takes no keyword arguments"
        )));
    }
    let [exception] = args else {
        return Err(type_error(format!(
            "{function}() takes exactly one argument ({} given)",
            args.len()
        )));
    };
    let name = function.strip_suffix("_errors").unwrap_or(function);
    if name == "strict" {
        if !matches!(ctx.heap.get(*exception), Value::Raised(_)) {
            return Err(type_error("codec must pass exception instance"));
        }
        return ctx.op("raise", *exception, NONE, NONE);
    }
    let kind = ctx.type_name(*exception);
    if kind != "UnicodeDecodeError" || matches!(name, "xmlcharrefreplace" | "namereplace") {
        return Err(type_error(format!(
            "don't know how to handle {kind} in error callback"
        )));
    }
    let object = ctx.lookup_attr(*exception, "object")?;
    let start = ctx.lookup_attr(*exception, "start")?;
    let end = ctx.lookup_attr(*exception, "end")?;
    let (start, end) = (ctx.as_integer(start)?, ctx.as_integer(end)?);
    let (pass_utf8, pass_wide) = if name == "surrogatepass" {
        let encoding = ctx.lookup_attr(*exception, "encoding")?;
        // The C codec handler reads the encoding name up to its first NUL.
        let encoding = ctx
            .str_value(encoding)?
            .split('\0')
            .next()
            .unwrap_or_default();
        let utf8 = ["utf-8", "utf_8", "utf8", "cp65001"]
            .iter()
            .any(|key| encoding.eq_ignore_ascii_case(key));
        (utf8, wide_unicode::surrogate_encoding(encoding))
    } else {
        (false, None)
    };
    let Value::Bytes(bytes) = ctx.heap.get(ctx.scalar_value(object)) else {
        return Err(type_error(
            "UnicodeError 'object' attribute must be a bytes",
        ));
    };
    let len = bytes.len() as i64;
    let end = end.max(1).min(len) as usize;
    let start = start.max(0).min((len - 1).max(0)) as usize;
    let bad = bytes.get(start..end).unwrap_or_default();
    if name == "surrogatepass" {
        // The validator stops at the leading byte. The handler inspects the
        // complete three-byte representation, even when end - start == 1.
        let replacement = if pass_utf8
            && let Some([0xed, b @ 0xa0..=0xbf, c @ 0x80..=0xbf]) = bytes.get(start..start + 3)
        {
            let codepoint = 0xd000 + (u32::from(*b & 0x3f) << 6) + u32::from(*c & 0x3f);
            Some((codepoint, 3))
        } else {
            pass_wide.and_then(|format| wide_unicode::surrogate(format, &bytes[start..]))
        };
        if let Some((codepoint, width)) = replacement {
            let text = ctx.text_codepoints(vec![codepoint]);
            let index = ctx.alloc(Value::Int(crate::Int::from_usize(start + width)));
            return Ok(ctx.alloc(Value::Tuple(vec![text, index])));
        }
        return ctx.op("raise", *exception, NONE, NONE);
    }
    if name == "surrogateescape" && bad.first().is_none_or(u8::is_ascii) {
        return ctx.op("raise", *exception, NONE, NONE);
    }
    if name == "surrogateescape" {
        // CPython's handler consumes at most four consecutive non-ASCII bytes.
        let replacement: Vec<_> = bad
            .iter()
            .take(4)
            .take_while(|b| !b.is_ascii())
            .map(|&b| 0xdc00 + u32::from(b))
            .collect();
        let next = start + replacement.len();
        let text = ctx.text_codepoints(replacement);
        let index = ctx.alloc(Value::Int(crate::Int::from_usize(next)));
        return Ok(ctx.alloc(Value::Tuple(vec![text, index])));
    }
    let mut replacement = String::new();
    crate::objects::decoding_error(&mut replacement, Some(name), bad, || unreachable!())?;
    let value = ctx.text(replacement);
    let position = ctx.alloc(Value::Int(crate::Int::from_usize(end)));
    Ok(ctx.alloc(Value::Tuple(vec![value, position])))
}

fn initialize_search_path(ctx: &mut Context) -> Result<()> {
    if !ctx.codec_entries.initialized {
        ctx.codec_entries
            .search_path
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        let default = ctx.builtin("encodings.search_function");
        ctx.codec_entries.search_path.push(default);
        ctx.codec_entries.initialized = true;
    }
    Ok(())
}

pub(super) fn search_registry(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let callback = unary_arg(function, args, kwargs)?;
    initialize_search_path(ctx)?;
    if function == "register" {
        if !ctx.callable(callback) {
            return Err(type_error("argument must be callable"));
        }
        ctx.codec_entries
            .search_path
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        ctx.codec_entries.search_path.push(callback);
    } else if let Some(index) = ctx
        .codec_entries
        .search_path
        .iter()
        .position(|&item| item == callback)
    {
        ctx.codec_entries.search_path.remove(index);
        ctx.heap.weak_edge_removed(callback);
        // Detach the dictionary before releasing entries. Destructors may reenter
        // lookup while later entries of the old dictionary are still alive.
        let old = std::mem::take(&mut ctx.codec_entries.search_cache);
        if ctx.heap.observes_released_roots() {
            let mark = ctx.heap.temporary_mark();
            for &entry in old.values().rev().flatten() {
                ctx.heap.keep(entry);
            }
            let count = ctx.heap.temporary_mark() - mark;
            for (index, entry) in old.into_values().flatten().enumerate() {
                let remaining = mark + count - index - 1;
                ctx.heap.release_temporaries_since(remaining);
                ctx.heap.weak_edge_removed(entry);
                ctx.collect_unreachable();
            }
            ctx.heap.release_temporaries_since(mark);
        }
    }
    Ok(NONE)
}

fn unary_arg(name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{name}() takes no keyword arguments"
        )));
    }
    match args {
        [arg] => Ok(*arg),
        _ => Err(type_error(format!(
            "_codecs.{name}() takes exactly one argument ({} given)",
            args.len()
        ))),
    }
}

pub(super) fn lookup(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let name = unary_arg("lookup", args, kwargs)?;
    let storage = ctx.string_storage(name).ok_or_else(|| {
        let kind = if name == NONE {
            "None"
        } else {
            ctx.type_name(name)
        };
        type_error(format!("lookup() argument must be str, not {kind}"))
    })?;
    let spelling = match ctx.heap.get(storage) {
        Value::Str(text) => text.clone(),
        _ => String::from_utf8(ctx.encode_utf8_codepoints(storage, false, "strict")?)
            .expect("strict UTF-8 encoding"),
    };
    lookup_name(ctx, &spelling)
}

/// Search the native codec provider outside the registry and preserve its entry identity.
pub(super) fn native_search(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let arg = match (args, kwargs) {
        ([name], []) => *name,
        ([], [(keyword, name)]) if keyword == "encoding" => *name,
        ([_], [(keyword, _)]) if keyword == "encoding" => {
            return Err(type_error(
                "search_function() got multiple values for argument 'encoding'",
            ));
        }
        (_, [(keyword, _), ..]) => {
            return Err(type_error(format!(
                "search_function() got an unexpected keyword argument '{keyword}'"
            )));
        }
        ([], []) => {
            return Err(type_error(
                "search_function() missing 1 required positional argument: 'encoding'",
            ));
        }
        _ => {
            return Err(type_error(format!(
                "search_function() takes 1 positional argument but {} were given",
                args.len()
            )));
        }
    };
    let source = if let Some(storage) = ctx.string_storage(arg) {
        storage
    } else if matches!(ctx.heap.get(ctx.scalar_value(arg)), Value::Bytes(_)) {
        decode_bytes(ctx, Codec::Ascii, arg, None)?
    } else {
        return Err(type_error(format!(
            "'{}' object is not iterable",
            ctx.type_name(arg)
        )));
    };
    // encodings.normalize_encoding preserves case. The registry has already
    // lowercased its argument before invoking this provider.
    let mut key = String::new();
    let mut punct = false;
    for &point in ctx.string_codepoints(source)?.iter() {
        if char::from_u32(point).is_some_and(|c| c.is_alphanumeric() || c == '.') {
            if punct && !key.is_empty() {
                key.push('_');
            }
            if point < 128 {
                key.push(point as u8 as char);
            }
            punct = false;
        } else {
            punct = true;
        }
    }
    if key.bytes().any(|c| c.is_ascii_uppercase()) {
        return Ok(NONE);
    }
    if let Some((format, true)) = FixedUnicode::encoding(&key) {
        let module = match format {
            FixedUnicode::Utf16(_) => "encodings.utf_16",
            _ => "encodings.utf_32",
        };
        let provider = ctx.module(module)?;
        let package = ctx.module("encodings")?;
        let entry = ctx.lookup_attr(package, "_wide_entry")?;
        return ctx.call(entry, vec![arg, provider], Vec::new());
    }
    match CodecInfo::lookup(&key).map_err(Error::from) {
        Ok(entry) => box_native_entry(ctx, entry),
        Err(error) if error.kind == "LookupError" => Ok(NONE),
        Err(error) => Err(error),
    }
}

pub(in crate::dynamic) fn lookup_name(ctx: &mut Context, spelling: &str) -> Result<Id> {
    Ok(lookup_transform(ctx, spelling, false)?.expect("Python lookup requires a codec entry"))
}

/// A transform with no boxed entry can use its existing native implementation only
/// when the ordered search reaches the default provider.
pub(super) fn lookup_transform(
    ctx: &mut Context,
    spelling: &str,
    native_fallback: bool,
) -> Result<Option<Id>> {
    if spelling.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    initialize_search_path(ctx)?;
    let key = crate::objects::normalize_codec(spelling)?;
    // The default provider can win for a native-only codec even when the requested
    // operation has no boxed implementation. A later callback cannot replace that win.
    let result_for_request = |entry: Option<Id>| {
        if entry.is_none() && !native_fallback {
            Err(Error::new(
                "LookupError",
                format!("unknown encoding: {spelling}"),
            ))
        } else {
            Ok(entry)
        }
    };
    if let Some(&entry) = ctx.codec_entries.search_cache.get(&key) {
        return result_for_request(entry);
    }
    let count = ctx.codec_entries.search_path.len();
    if count == 0 {
        return Err(Error::new(
            "LookupError",
            "no codec search functions registered: can't find encoding",
        ));
    }
    let name = ctx.text(key.clone());
    let (heap, roots) = ctx.heap_and_roots();
    let name = heap.intern(name, roots);
    let native_only = FixedUnicode::by_key(&key).is_some() || key == "raw_unicode_escape";
    let default = native_only.then(|| ctx.builtin("encodings.search_function"));
    // A callback may change the live list. CPython snapshots only its length.
    for index in 0..count {
        let callback = ctx
            .codec_entries
            .search_path
            .get(index)
            .copied()
            .ok_or_else(|| Error::new("IndexError", "list index out of range"))?;
        if Some(callback) == default {
            return result_for_request(cache_lookup(ctx, key, None)?);
        }
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(callback);
        let result = ctx.call(callback, vec![name], Vec::new());
        ctx.heap.release_temporaries_since(mark);
        match &result {
            Ok(entry) => ctx.heap.keep(*entry),
            Err(error) => {
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
            }
        }
        if ctx.heap.observes_released_roots() && !ctx.codec_entries.search_path.contains(&callback)
        {
            ctx.heap.weak_edge_removed(callback);
            ctx.collect_unreachable();
        }
        let entry = result?;
        if entry == NONE {
            continue;
        }
        if !ctx
            .tuple_items(entry)?
            .is_some_and(|items| items.len() == 4)
        {
            return Err(type_error("codec search functions must return 4-tuples"));
        }
        return cache_lookup(ctx, key, Some(entry));
    }
    Err(Error::new(
        "LookupError",
        format!("unknown encoding: {spelling}"),
    ))
}

fn cache_lookup(ctx: &mut Context, key: String, entry: Option<Id>) -> Result<Option<Id>> {
    ctx.codec_entries
        .search_cache
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    // Successful normalized names, unlike ordinary misses, are immortal in CPython.
    ctx.heap.code_name(&key);
    if let Some(Some(old)) = ctx.codec_entries.search_cache.insert(key, entry) {
        ctx.heap.weak_edge_removed(old);
        if Some(old) != entry && ctx.heap.observes_released_roots() {
            ctx.collect_unreachable();
        }
    }
    Ok(entry)
}

/// Fetch the stateless native transforms without searching the Python registry during import.
pub(super) fn entry_functions(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let name = unary_arg("_imre_codec_functions", args, kwargs)?;
    let codec = Codec::lookup(ctx.str_value(name)?)?;
    let (encode, decode) = native_functions(ctx, codec)?;
    Ok(ctx.alloc(Value::Tuple(vec![encode, decode])))
}

fn native_functions(ctx: &mut Context, codec: Codec) -> Result<(Id, Id)> {
    if let Some(&functions) = ctx.codec_entries.functions.get(codec.name()) {
        return Ok(functions);
    }
    ctx.codec_entries
        .functions
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    let functions = (
        ctx.alloc(Value::CodecEncoder(codec)),
        ctx.alloc(Value::CodecDecoder(codec)),
    );
    ctx.codec_entries.functions.insert(codec.name(), functions);
    Ok(functions)
}

fn box_native_entry(ctx: &mut Context, entry: CodecInfo) -> Result<Id> {
    let key = entry.as_ptr();
    if let Some((_, id)) = ctx.codec_entries.entries.get(&key) {
        return Ok(*id);
    }
    let codec = entry.codec();
    let suffix = match codec {
        Codec::Ascii => "ascii",
        Codec::Latin1 => "latin_1",
        Codec::Utf8 { signature: false } => "utf_8",
        Codec::Utf8 { signature: true } => "utf_8_sig",
        Codec::CodePage(page) => page.module_name,
        Codec::ShiftJis => codec.name(),
    };
    let module = ctx.module(&format!("encodings.{suffix}"))?;
    // Keep the pending constructor arguments across Python-visible module lookups.
    ctx.heap.reserve_temporaries(6)?;
    ctx.heap.keep(module);
    let codecs = ctx.lookup_attr(module, "codecs")?;
    let class = ctx.lookup_attr(codecs, "CodecInfo")?;
    ctx.heap.keep(class);
    let writer = ctx.lookup_attr(module, "StreamWriter")?;
    ctx.heap.keep(writer);
    let (encode, decode) = match codec {
        Codec::CodePage(_) | Codec::ShiftJis => {
            // getregentry creates fresh bound Codec methods for each alias entry.
            let class = ctx.lookup_attr(module, "Codec")?;
            let encoder = ctx.call(class, Vec::new(), Vec::new())?;
            let decoder = ctx.call(class, Vec::new(), Vec::new())?;
            (
                ctx.lookup_attr(encoder, "encode")?,
                ctx.lookup_attr(decoder, "decode")?,
            )
        }
        Codec::Utf8 { signature: true } => {
            let (_, decode) = native_functions(ctx, codec)?;
            (ctx.lookup_attr(module, "encode")?, decode)
        }
        _ => native_functions(ctx, codec)?,
    };
    ctx.heap.keep(encode);
    ctx.heap.keep(decode);
    // getregentry resolves the factories as globals in the encoding module.
    let factory = |ctx: &mut Context, name: &str| match *ctx.heap.get(module) {
        Value::Module { scope, .. } => ctx.lookup(scope, name),
        _ => ctx.lookup_attr(module, name),
    };
    let encoder = if matches!(
        codec,
        Codec::Ascii | Codec::Latin1 | Codec::ShiftJis | Codec::Utf8 { .. }
    ) {
        factory(ctx, "IncrementalEncoder")?
    } else {
        NONE
    };
    ctx.heap.keep(encoder);
    let decoder = factory(ctx, "IncrementalDecoder")?;
    let name = ctx.text(codec.name());
    let id = ctx.call(
        class,
        Vec::new(),
        vec![
            ("name".into(), name),
            ("encode".into(), encode),
            ("decode".into(), decode),
            ("incrementalencoder".into(), encoder),
            ("incrementaldecoder".into(), decoder),
            ("streamreader".into(), NONE),
            ("streamwriter".into(), writer),
        ],
    )?;
    ctx.codec_entries
        .entries
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    ctx.codec_entries.entries.insert(key, (entry, id));
    Ok(id)
}

pub(in crate::dynamic) fn encode_text(
    ctx: &mut Context,
    id: Id,
    encoding_id: Option<Id>,
    errors_id: Option<Id>,
) -> Result<Vec<u8>> {
    let encoding = encoding_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    let errors = errors_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    let encoder = StrEncoder::lookup(encoding, errors)?;
    let errors = if let StrEncoder::Registered(codec) = encoder {
        match encode_error::registered(ctx, codec, errors) {
            Some(encode_error::Handler::Python(callback)) => {
                // A scalar string already contains its valid UTF-8 encoding.
                if matches!(codec, Codec::Utf8 { .. })
                    && let Ok(text) = ctx.str_value(id)
                {
                    return encoder
                        .encode_str(text, errors)
                        .map_err(|error| ctx.encode_error(error, id));
                }
                let errors = errors.to_owned();
                return encode_error::encode(ctx, id, codec, callback, &errors);
            }
            Some(encode_error::Handler::Builtin(name)) => name,
            None => errors,
        }
    } else {
        errors
    };
    if let Ok(text) = ctx.str_value(id) {
        return encoder
            .encode_str(text, errors)
            .map_err(|error| ctx.encode_error(error, id));
    }
    let errors = errors.to_owned();
    match encoder {
        StrEncoder::Wide { format, signature } => {
            let order = if signature { 0 } else { format.byte_order() };
            encode_fixed(ctx, id, format, &errors, order)
        }
        StrEncoder::Registered(Codec::Utf8 { signature }) => {
            ctx.encode_utf8_codepoints(id, signature, &errors)
        }
        StrEncoder::Registered(codec)
            if matches!(
                errors.as_str(),
                "strict" | "surrogateescape" | "surrogatepass"
            ) =>
        {
            ctx.encode_legacy_codepoints(id, codec, errors == "surrogateescape")
        }
        StrEncoder::Registered(codec)
            if matches!(
                errors.as_str(),
                "ignore" | "replace" | "backslashreplace" | "namereplace" | "xmlcharrefreplace"
            ) =>
        {
            // Apply the built-in handler to lone surrogates, then encode the scalar result.
            let replaced = ctx.encode_utf8_codepoints(id, false, &errors)?;
            let text = std::str::from_utf8(&replaced).expect("these handlers produce scalar text");
            Ok(StrEncoder::Registered(codec).encode_str(text, &errors)?)
        }
        _ => Err(Error::new(
            "NotImplementedError",
            "encoding non-scalar strings with this codec is unsupported",
        )),
    }
}

/// Fixed-endian encoders report one surrogate at a time, including consecutive surrogates.
pub(super) fn encode_fixed(
    ctx: &mut Context,
    id: Id,
    encoder: FixedUnicode,
    errors: &str,
    order: i32,
) -> Result<Vec<u8>> {
    let encoder = encoder.with_order(order);
    let signature = order == 0;
    if let Ok(text) = ctx.str_value(id) {
        return Ok(StrEncoder::Wide {
            format: encoder,
            signature,
        }
        .encode_str(text, errors)?);
    }
    let points = ctx.string_codepoints(id)?;
    let mut bytes = Vec::new();
    if signature {
        encoder.append(&mut bytes, [0xfeff])?;
    }
    // CPython names UTF-32 encode errors using only exact -1/+1 orders.
    // Its surrogatepass handler uses that name to choose the replacement byte order.
    let generic = match encoder {
        FixedUnicode::Utf16(_) => signature,
        FixedUnicode::Utf32Le | FixedUnicode::Utf32Be => !matches!(order, -1 | 1),
    };
    let name = encoder.codec_name(generic);
    for (i, &point) in points.iter().enumerate() {
        if char::from_u32(point).is_some() {
            encoder.append(&mut bytes, [point])?;
            continue;
        }
        if ctx.codec_entries.handlers.contains_key(errors) {
            return Err(Error::new(
                "NotImplementedError",
                match encoder {
                    FixedUnicode::Utf16(_) => {
                        "custom UTF-16 encoding error handlers are unsupported"
                    }
                    FixedUnicode::Utf32Le | FixedUnicode::Utf32Be => {
                        "custom UTF-32 encoding error handlers are unsupported"
                    }
                },
            ));
        }
        match errors {
            "strict" | "surrogateescape" => {
                return Err(ctx.string_encoding_error(
                    id,
                    name,
                    "surrogates not allowed",
                    point,
                    i..i + 1,
                ));
            }
            "ignore" => (),
            "replace" => encoder.append(&mut bytes, [u32::from(b'?')])?,
            "surrogatepass" => {
                let format = if generic {
                    encoder.with_order(0)
                } else {
                    encoder
                };
                format.append(&mut bytes, [point])?;
            }
            "backslashreplace" | "namereplace" => encoder.append(
                &mut bytes,
                compact_str::format_compact!("\\u{point:04x}")
                    .chars()
                    .map(u32::from),
            )?,
            "xmlcharrefreplace" => encoder.append(
                &mut bytes,
                compact_str::format_compact!("&#{point};")
                    .chars()
                    .map(u32::from),
            )?,
            _ => return Err(crate::objects::unknown_handler(errors).into()),
        }
    }
    Ok(bytes)
}

mod transform;
pub(super) use transform::transform;
pub(in crate::dynamic) use transform::{decode_named, encode_text_object};

fn error_name(ctx: &Context, errors: Option<Id>) -> Result<&str> {
    let name = errors
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    if name.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    Ok(name)
}

/// Regular codecs share the typed implementation. Registered Python callbacks reenter the native heap.
pub(in crate::dynamic) fn decode_bytes(
    ctx: &mut Context,
    codec: Codec,
    input: Id,
    errors: Option<Id>,
) -> Result<Id> {
    let name = error_name(ctx, errors)?;
    if matches!(codec, Codec::ShiftJis) {
        let name = name.to_owned();
        return ctx.with_buffer_export(input, |ctx| {
            decode_shift_jis(ctx, input, &name, true).map(|(text, _)| text)
        });
    }
    if matches!(codec, Codec::Ascii) && name == "surrogateescape" {
        let bytes = ctx.bytes_like(input)?;
        if !bytes.is_ascii() {
            return Ok(ctx.text_codepoints(codec.decode_filesystem(bytes)?));
        }
    }
    if let Codec::Utf8 { signature } = codec
        && name == "surrogateescape"
    {
        let bytes = ctx.bytes_like(input)?;
        let bytes = if signature {
            bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
        } else {
            bytes
        };
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Ok(ctx.text(text.to_owned()));
        }
        let points = crate::objects::interpreter_stdlib::fsdecode(bytes).collect();
        return Ok(ctx.text_codepoints(points));
    }
    if let Codec::Utf8 { signature } = codec
        && uses_python_handler(ctx, name)
    {
        let name = name.to_owned();
        return ctx.with_buffer_export(input, |ctx| registered_utf8(ctx, input, &name, signature));
    }
    let result = codec
        .decode(ctx.bytes_like(input)?, Some(name))
        .map_err(|err| ctx.decode_error(err))?;
    Ok(ctx.text(result))
}

pub(super) fn uses_python_handler(ctx: &Context, name: &str) -> bool {
    name == "surrogatepass"
        || !matches!(name, "ignore" | "replace" | "surrogateescape")
            && (handler_builtin(name).is_none() || ctx.codec_entries.handlers.contains_key(name))
}

/// URL decoding creates bytes for a single percent-decoded portion of a component.
pub(super) fn decode_slice(
    ctx: &mut Context,
    codec: Codec,
    data: &[u8],
    errors: &str,
) -> Result<String> {
    if let Codec::Utf8 { signature } = codec
        && uses_python_handler(ctx, errors)
    {
        if let Ok(valid) = std::str::from_utf8(data) {
            return Ok(if signature {
                valid.strip_prefix('\u{feff}').unwrap_or(valid)
            } else {
                valid
            }
            .to_owned());
        }
        let input = ctx.alloc(Value::Bytes(data.to_vec()));
        let result = registered_utf8(ctx, input, errors, signature)?;
        Ok(ctx.str_value(result)?.to_owned())
    } else {
        codec
            .decode(data, Some(errors))
            .map_err(|e| ctx.decode_error(e))
    }
}

fn registered_utf8(ctx: &mut Context, input: Id, name: &str, signature: bool) -> Result<Id> {
    stateful_utf8(ctx, input, name, signature, true).map(|(text, _)| text)
}

/// Consume complete sequences and leave a trailing partial character for the next call.
fn stateful_utf8(
    ctx: &mut Context,
    input: Id,
    name: &str,
    signature: bool,
    final_input: bool,
) -> Result<(Id, usize)> {
    let data = ctx.native_buffer_bytes(input)?;
    // The utf-8-sig codec passes the slice after the BOM to utf_8_decode.
    let data = if signature {
        data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
    } else {
        data
    };
    let mut consumed = 0;
    let mut prefix =
        |text: &mut String, data: &[u8], offset: usize| -> Result<Option<DecodeIssue>> {
            let issue = crate::objects::utf8_prefix(text, data, offset)?;
            if let Some(problem) = &issue
                && !final_input
            {
                let tail = &data[problem.start..];
                // A truncated surrogate may still be consumed by the surrogatepass handler.
                if problem.reason == "unexpected end of data" || matches!(tail, [0xed, 0xa0..=0xbf])
                {
                    consumed = problem.start;
                    return Ok(None);
                }
            }
            consumed = data.len();
            Ok(issue)
        };
    let mut text = String::new();
    let Some(issue) = prefix(&mut text, data, 0)? else {
        return Ok((ctx.text(text), consumed));
    };
    let points = text.chars().map(u32::from).collect();
    let error = issue.error(data);
    // CPython's UTF-8 decoder handles these policies directly even when
    // register_error has replaced a callback with the same name.
    let resolve = |ctx: &mut Context| match name {
        "ignore" | "replace" | "surrogateescape" => Ok(ctx.builtin(handler_builtin(name).unwrap())),
        _ => handler(ctx, name),
    };
    let result = resume_decode_with(
        ctx,
        resolve,
        points,
        error,
        None,
        |points, data, position| {
            text.clear();
            let issue = prefix(&mut text, data, position)?;
            points
                .try_reserve(text.len())
                .map_err(|_| crate::Error::allocation())?;
            points.extend(text.chars().map(u32::from));
            Ok(issue)
        },
    )?;
    Ok((result, consumed))
}

/// Decode a Shift JIS chunk, retaining an unpaired lead byte until the final call.
fn decode_shift_jis(
    ctx: &mut Context,
    input: Id,
    name: &str,
    final_input: bool,
) -> Result<(Id, usize)> {
    let data = ctx.native_buffer_bytes(input)?;
    let size = data.len();
    let mut text = String::new();
    let (mut consumed, issue) =
        crate::objects::shift_jis::decode_prefix(&mut text, data, 0, final_input)?;
    let Some(issue) = issue else {
        return Ok((ctx.text(text), consumed));
    };
    let error = issue.error(data);
    if name == "strict" {
        return Err(ctx.decode_error(error));
    }
    let points = text.chars().map(u32::from).collect();
    let resolve = |ctx: &mut Context| match name {
        "ignore" | "replace" => Ok(ctx.builtin(handler_builtin(name).unwrap())),
        _ => handler(ctx, name),
    };
    let result = resume_decode_with(
        ctx,
        resolve,
        points,
        error,
        Some(input),
        |points, data, position| {
            text.clear();
            let (offset, issue) =
                crate::objects::shift_jis::decode_prefix(&mut text, data, position, final_input)?;
            consumed = offset;
            points
                .try_reserve(text.len())
                .map_err(|_| crate::Error::allocation())?;
            points.extend(text.chars().map(u32::from));
            Ok(issue)
        },
    )?;
    Ok((result, if final_input { size } else { consumed }))
}

/// Resume from the position and possibly changed byte object supplied by the handler.
fn resume_decode(
    ctx: &mut Context,
    name: &str,
    points: Vec<u32>,
    error: crate::Error,
    prefix: impl FnMut(&mut Vec<u32>, &[u8], usize) -> Result<Option<DecodeIssue>>,
) -> Result<Id> {
    resume_decode_with(ctx, |ctx| handler(ctx, name), points, error, None, prefix)
}

fn resume_decode_with(
    ctx: &mut Context,
    resolve: impl FnOnce(&mut Context) -> Result<Id>,
    mut points: Vec<u32>,
    error: crate::Error,
    original: Option<Id>,
    mut prefix: impl FnMut(&mut Vec<u32>, &[u8], usize) -> Result<Option<DecodeIssue>>,
) -> Result<Id> {
    let raised = ctx.decode_error(error);
    if raised.decode.is_none() {
        return Err(raised);
    }
    let exception = ctx.alloc(Value::Raised(raised));
    let callback = resolve(ctx)?;
    // One decode retains its first handler even if a callback replaces the registry entry.
    ctx.heap.keep(callback);
    loop {
        let response = ctx.call(callback, vec![exception], Vec::new())?;
        let invalid = || type_error("decoding error handler must return (str, int) tuple");
        let values = ctx.tuple_items(response)?.ok_or_else(invalid)?;
        let [replacement, newpos] = values.as_slice() else {
            return Err(invalid());
        };
        let (replacement, newpos) = (*replacement, *newpos);
        let replacement = ctx.string_codepoints(replacement).map_err(|_| invalid())?;
        points
            .try_reserve(replacement.len())
            .map_err(|_| crate::Error::allocation())?;
        points.extend_from_slice(&replacement);
        let position = ctx
            .index_integer(newpos)?
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
        let data = if let Some(input) = original {
            // The multibyte codec resumes in its original buffer if a handler rebinds exc.object.
            ctx.native_buffer_bytes(input)?
        } else {
            let input = ctx.lookup_attr(exception, "object")?;
            let Value::Bytes(data) = ctx.heap.get(ctx.scalar_value(input)) else {
                return Err(type_error(
                    "UnicodeError 'object' attribute must be a bytes",
                ));
            };
            data
        };
        let length = data.len() as isize;
        let position = if position < 0 {
            length + position
        } else {
            position
        };
        if !(0..=length).contains(&position) {
            return Err(Error::new(
                "IndexError",
                format!("position {position} from error handler out of bounds"),
            ));
        }
        let Some(issue) = prefix(&mut points, data, position as usize)? else {
            return Ok(ctx.text_codepoints(points));
        };
        ctx.advance_decode_error(exception, issue.start, issue.end, issue.reason);
    }
}

fn positional_codec_args(
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, Option<Id>)> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    match args {
        [input] => Ok((*input, None)),
        [input, errors] => Ok((*input, Some(*errors))),
        [] => Err(type_error(format!(
            "{function} expected at least 1 argument, got 0"
        ))),
        _ => Err(type_error(format!(
            "{function} expected at most 2 arguments, got {}",
            args.len()
        ))),
    }
}

pub(super) fn utf8_decode(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(
            "_codecs.utf_8_decode() takes no keyword arguments",
        ));
    }
    let Some(&input) = args.first() else {
        return Err(type_error(
            "utf_8_decode expected at least 1 argument, got 0",
        ));
    };
    if args.len() > 3 {
        return Err(type_error(format!(
            "utf_8_decode expected at most 3 arguments, got {}",
            args.len()
        )));
    }
    ctx.with_buffer_export(input, |ctx| {
        ctx.native_buffer_bytes(input)?;
        let errors = entry_error_policy(ctx, "utf_8_decode", args.get(1).copied())?;
        let final_input = args
            .get(2)
            .map(|&id| ctx.truth(id))
            .transpose()?
            .unwrap_or(false);
        let name = error_name(ctx, errors)?.to_owned();
        let (text, consumed) = stateful_utf8(ctx, input, &name, false, final_input)?;
        let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
        Ok(ctx.alloc(Value::Tuple(vec![text, consumed])))
    })
}

pub(super) fn escape_decode(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (input, errors) = positional_codec_args("escape_decode", args, kwargs)?;
    ctx.with_buffer_export(input, |ctx| bytes_escape::decode(ctx, input, errors))
}

/// Bind the Python encoder/decoder wrappers, including a bound method's implicit self.
fn python_codec_args(
    wrapper: &str,
    bound: bool,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, Option<Id>)> {
    let mut input = args.first().copied();
    let mut errors = args.get(1).copied();
    for (key, value) in kwargs {
        let slot = match key.as_str() {
            "input" => &mut input,
            "errors" => &mut errors,
            _ => {
                return Err(type_error(format!(
                    "{wrapper}() got an unexpected keyword argument '{key}'"
                )));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(type_error(format!(
                "{wrapper}() got multiple values for argument '{key}'"
            )));
        }
    }
    if args.len() > 2 {
        let offset = usize::from(bound);
        return Err(type_error(format!(
            "{wrapper}() takes from {} to {} positional arguments but {} were given",
            1 + offset,
            2 + offset,
            args.len() + offset
        )));
    }
    Ok((
        input.ok_or_else(|| {
            type_error(format!(
                "{wrapper}() missing 1 required positional argument: 'input'"
            ))
        })?,
        errors,
    ))
}

fn entry_error_policy(ctx: &Context, function: &str, errors: Option<Id>) -> Result<Option<Id>> {
    Ok(match errors {
        None | Some(NONE) => None,
        Some(id) => {
            let Value::Str(name) = ctx.heap.get(ctx.scalar_value(id)) else {
                return Err(type_error(format!(
                    "{function}() argument 2 must be str or None, not {}",
                    ctx.type_name(id)
                )));
            };
            if name.contains('\0') {
                return Err(Error::new("ValueError", "embedded null character"));
            }
            Some(id)
        }
    })
}

fn encoder_input_length(ctx: &Context, function: &str, input: Id) -> Result<usize> {
    Ok(match ctx.heap.get(ctx.scalar_value(input)) {
        Value::Str(text) => text.chars().count(),
        Value::SurrogateStr(points) => points.len(),
        _ => {
            return Err(type_error(format!(
                "{function}() argument 1 must be str, not {}",
                ctx.type_name(input)
            )));
        }
    })
}

/// Call the encoder stored on a built-in registry entry and report characters consumed.
pub(in crate::dynamic) fn entry_encode(
    ctx: &mut Context,
    codec: Codec,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let function = match codec {
        Codec::Latin1 => "latin_1_encode",
        Codec::Ascii => "ascii_encode",
        Codec::Utf8 { .. } => "utf_8_encode",
        Codec::CodePage(_) => "charmap_encode",
        Codec::ShiftJis => "encode",
    };
    let (input, errors) = if matches!(codec, Codec::Utf8 { signature: true } | Codec::CodePage(_)) {
        let wrapper = if matches!(codec, Codec::CodePage(_)) {
            "Codec.encode"
        } else {
            "encode"
        };
        python_codec_args(wrapper, matches!(codec, Codec::CodePage(_)), args, kwargs)?
    } else if matches!(codec, Codec::ShiftJis) {
        python_codec_args("encode", false, args, kwargs)?
    } else {
        positional_codec_args(function, args, kwargs)?
    };
    // The multibyte codec converts non-string inputs with str(input).
    let input = if matches!(codec, Codec::ShiftJis) && ctx.string_storage(input).is_none() {
        crate::dynamic::builtins::call(ctx, "str", &[input], &[])?
    } else {
        input
    };
    let length = encoder_input_length(ctx, function, input)?;
    let encoding = ctx.text(codec.name());
    let policy = entry_error_policy(ctx, function, errors)?;
    let data = encode_text(ctx, input, Some(encoding), policy)?;
    let data = ctx.alloc(Value::Bytes(data));
    let length = ctx.int(length as i64);
    Ok(ctx.alloc(Value::Tuple(vec![data, length])))
}

pub(in crate::dynamic) fn decode(
    ctx: &mut Context,
    codec: Codec,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let function = match codec {
        Codec::Latin1 => "latin_1_decode",
        Codec::Ascii => "ascii_decode",
        Codec::Utf8 { .. } => "utf_8_decode",
        Codec::CodePage(_) => "charmap_decode",
        Codec::ShiftJis => "decode",
    };
    if matches!(codec, Codec::ShiftJis) {
        // The module's incremental decoder passes final as a third argument.
        let (arguments, final_input) = match args {
            [_, _, final_input] => (&args[..2], ctx.truth(*final_input)?),
            _ => (args, true),
        };
        let (input, errors) = python_codec_args("decode", false, arguments, kwargs)?;
        let policy = entry_error_policy(ctx, function, errors)?;
        let name = error_name(ctx, policy)?.to_owned();
        return ctx.with_buffer_export(input, |ctx| {
            let (text, size) = decode_shift_jis(ctx, input, &name, final_input)?;
            let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(size)));
            Ok(ctx.alloc(Value::Tuple(vec![text, consumed])))
        });
    }
    let (input, errors) = match codec {
        Codec::Latin1 | Codec::Ascii => positional_codec_args(function, args, kwargs)?,
        _ => {
            let bound = matches!(codec, Codec::CodePage(_));
            let wrapper = if bound { "Codec.decode" } else { "decode" };
            python_codec_args(wrapper, bound, args, kwargs)?
        }
    };
    let consumed = ctx.bytes_like(input)?.len();
    let policy = entry_error_policy(ctx, function, errors)?;
    let text = decode_bytes(ctx, codec, input, policy)?;
    let length = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
    Ok(ctx.alloc(Value::Tuple(vec![text, length])))
}
