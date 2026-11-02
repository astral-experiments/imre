//! Native I/O classes for runtime-loaded modules.

use super::super::builtins::zeroed_bytes;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

mod buffered_io;
mod buffered_reader;
mod descriptor_buffer;
pub(in crate::dynamic) use descriptor_buffer::BufferedFd;
mod bytes_io;
mod file_io;
pub(in crate::dynamic) use file_io::open;
mod text_wrapper;

#[derive(Clone)]
pub(in crate::dynamic) enum MemoryStream {
    Text(crate::objects::TextFile),
    Reader(std::rc::Rc<buffered_reader::Reader>),
    UninitializedReader(Option<Id>),
    Bytes(std::rc::Rc<bytes_io::BytesIo>),
    Wrapper(std::rc::Rc<text_wrapper::Wrapper>),
    Raw(std::rc::Rc<file_io::RawFile>),
}

const DEFAULT_BUFFER_SIZE: i64 = 128 * 1024;

// CPython 3.14.5 Lib/io.py, in wildcard import order.
const PUBLIC_NAMES: &[&str] = &[
    "BlockingIOError",
    "open",
    "open_code",
    "IOBase",
    "RawIOBase",
    "FileIO",
    "BytesIO",
    "StringIO",
    "BufferedIOBase",
    "BufferedReader",
    "BufferedWriter",
    "BufferedRWPair",
    "BufferedRandom",
    "TextIOBase",
    "TextIOWrapper",
    "UnsupportedOperation",
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "DEFAULT_BUFFER_SIZE",
    "text_encoding",
    "IncrementalNewlineDecoder",
    "Reader",
    "Writer",
];

/// Select the default text encoding from the interpreter's startup configuration.
pub(super) fn text_encoding(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_io.text_encoding() takes no keyword arguments",
        ));
    }
    let (encoding, level) = match args {
        [encoding] => (*encoding, 2),
        [encoding, level] => {
            let level = ctx
                .index_integer(*level)?
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?;
            (*encoding, level)
        }
        [] => {
            return Err(Error::new(
                "TypeError",
                "text_encoding expected at least 1 argument, got 0",
            ));
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "text_encoding expected at most 2 arguments, got {}",
                    args.len()
                ),
            ));
        }
    };
    if encoding != NONE {
        return Ok(encoding);
    }
    if ctx.config["warn_default_encoding"].as_bool() == Some(true) {
        ctx.encoding_warning(level)?;
    }
    let name = if ctx.config["utf8_mode"].as_bool() == Some(true) {
        "utf-8"
    } else {
        "locale"
    };
    Ok(ctx.heap.code_name(name))
}

pub(super) fn open_code(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 1 {
        return Err(Error::new(
            "TypeError",
            format!("open_code() takes at most 1 argument ({count} given)"),
        ));
    }
    let path = match (args, kwargs) {
        ([path], []) => *path,
        ([], [(name, path)]) if name == "path" => *path,
        _ => {
            return Err(Error::new(
                "TypeError",
                "open_code() missing required argument 'path' (pos 1)",
            ));
        }
    };
    if ctx.string_storage(path).is_none() {
        let kind = if path == NONE {
            "None"
        } else {
            ctx.type_name(path)
        };
        return Err(Error::new(
            "TypeError",
            format!("open_code() argument 'path' must be str, not {kind}"),
        ));
    }
    let module = ctx.module("_io")?;
    let open = ctx.lookup_attr(module, "open")?;
    let mode = ctx.text("rb");
    ctx.call(open, vec![path, mode], Vec::new())
}

/// Install canonical class objects so subclasses inherit the same methods across imports.
pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let names = PUBLIC_NAMES.iter().map(|&name| ctx.text(name)).collect();
    let exports = ctx.alloc(Value::List(names));
    ctx.bind(module, "__all__", exports);
    let open = ctx.builtin("open");
    ctx.bind(module, "open", open);
    let text_encoding = ctx.builtin("_io.text_encoding");
    ctx.bind(module, "text_encoding", text_encoding);
    let unsupported = ctx.builtin("UnsupportedOperation");
    ctx.bind(module, "UnsupportedOperation", unsupported);
    let size = ctx.int(DEFAULT_BUFFER_SIZE);
    ctx.bind(module, "DEFAULT_BUFFER_SIZE", size);
    for (name, origin) in [("SEEK_SET", 0), ("SEEK_CUR", 1), ("SEEK_END", 2)] {
        let value = ctx.int(origin);
        ctx.bind(module, name, value);
    }
    let object = ctx.builtin("object");
    let io = class(
        ctx,
        module,
        "IOBase",
        object,
        &[
            "closed",
            "__del__",
            "__iter__",
            "__next__",
            "readlines",
            "writelines",
            "close",
            "flush",
            "readable",
            "writable",
            "seekable",
            "__enter__",
            "__exit__",
        ],
    );
    let raw = class(
        ctx,
        module,
        "RawIOBase",
        io,
        &["read", "readall", "readinto", "write"],
    );
    ctx.builtins.insert("_io._RawIOBase".into(), raw);
    let file = class(ctx, module, "FileIO", raw, file_io::METHODS);
    ctx.builtins.insert("_io.FileIO".into(), file);
    let buffered = class(ctx, module, "BufferedIOBase", io, buffered_io::METHODS);
    ctx.builtins.insert("_io._BufferedIOBase".into(), buffered);
    let reader = class(
        ctx,
        module,
        "BufferedReader",
        buffered,
        buffered_reader::METHODS,
    );
    ctx.builtins.insert("_io.BufferedReader".into(), reader);
    let bytes_io = class(ctx, module, "BytesIO", buffered, bytes_io::METHODS);
    ctx.builtins.insert("_io.BytesIO".into(), bytes_io);
    let text = class(
        ctx,
        module,
        "TextIOBase",
        io,
        &["encoding", "errors", "newlines", "detach"],
    );
    ctx.builtins.insert("_io._TextIOBase".into(), text);
    let wrapper = class(ctx, module, "TextIOWrapper", text, text_wrapper::METHODS);
    ctx.builtins.insert("_io.TextIOWrapper".into(), wrapper);
    let string_io = class(
        ctx,
        module,
        "StringIO",
        text,
        &[
            "__init__",
            "write",
            "getvalue",
            "read",
            "readline",
            "seek",
            "tell",
            "truncate",
            "isatty",
            "newlines",
            "line_buffering",
        ],
    );
    ctx.builtins.insert("_io.StringIO".into(), string_io);
}

fn class(ctx: &mut Context, module: Id, name: &str, base: Id, methods: &[&'static str]) -> Id {
    let scope = ctx.scope("io", Some(module));
    let owner = ctx
        .allocate_class(name.into(), scope, vec![base])
        .expect("a single I/O base has a linearization");
    let module_name = ctx.text(
        if matches!(
            name,
            "StringIO" | "BytesIO" | "TextIOWrapper" | "BufferedReader" | "FileIO"
        ) {
            "_io"
        } else {
            "io"
        },
    );
    ctx.bind(scope, "__module__", module_name);
    let qualname = ctx.text(name);
    ctx.bind(scope, "__qualname__", qualname);
    let wrapper = name == "TextIOWrapper";
    let reader = name == "BufferedReader";
    let descriptor_owner = match name {
        "TextIOBase" => "_io._TextIOBase",
        "StringIO" => "_io.StringIO",
        "FileIO" => "_io.FileIO",
        "TextIOWrapper" => "_io.TextIOWrapper",
        "BytesIO" => "_io.BytesIO",
        "BufferedReader" => "_io.BufferedReader",
        _ => "_io._IOBase",
    };
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod { owner, name });
        let attr = if matches!(
            name,
            "closed" | "encoding" | "errors" | "line_buffering" | "newlines"
        ) || descriptor_owner == "_io.FileIO"
            && matches!(name, "mode" | "closefd" | "_blksize")
            || wrapper && text_wrapper::property(name)
            || reader && matches!(name, "raw" | "mode" | "name")
        {
            let set = (wrapper && name == "_CHUNK_SIZE").then(|| {
                ctx.alloc(Value::IoMethod {
                    owner,
                    name: "set_chunk_size",
                })
            });
            ctx.alloc(Value::GetSetDescriptor {
                get: method,
                set,
                owner: descriptor_owner,
                name,
            })
        } else {
            method
        };
        ctx.bind(scope, name, attr);
    }
    ctx.bind(module, name, owner);
    owner
}

fn argument_count(owner: &str, name: &str, n: usize) -> Result<()> {
    let message = match name {
        "read" | "read1" | "readline" | "readlines" | "truncate" if n > 1 => {
            format!("{name} expected at most 1 argument, got {n}")
        }
        "seek" if n == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if n > 2 => format!("seek expected at most 2 arguments, got {n}"),
        "write" | "readinto" | "writelines" if n != 1 => {
            format!("{owner}.{name}() takes exactly one argument ({n} given)")
        }
        "read" | "read1" | "readline" | "readlines" | "writelines" | "seek" | "write"
        | "readinto" | "truncate" | "__exit__" => {
            return Ok(());
        }
        _ if n != 0 => format!("{owner}.{name}() takes no arguments ({n} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

/// IOBase consumes the receiver's actual iterator and each writer override.
fn lines(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("_IOBase.{name}() takes no keyword arguments"),
        ));
    }
    argument_count("_IOBase", name, args.len())?;
    if matches!(name, "__iter__" | "writelines") {
        let closed = ctx.lookup_attr(receiver, "closed")?;
        if ctx.truth(closed)? {
            return Err(Error::new("ValueError", "I/O operation on closed file."));
        }
    }
    match name {
        "__iter__" => Ok(receiver),
        "__next__" => {
            let method = ctx.lookup_attr(receiver, "readline")?;
            let line = ctx.call(method, Vec::new(), Vec::new())?;
            let len = ctx.builtin("len");
            let length = ctx.call(len, vec![line], Vec::new())?;
            if ctx.truth(length)? {
                Ok(line)
            } else {
                Err(Error::new("StopIteration", ""))
            }
        }
        "writelines" => {
            let iterator = ctx.iterator(args[0])?;
            while let Some(line) = ctx.next_item(iterator)? {
                let write = ctx.lookup_attr(receiver, "write")?;
                ctx.call(write, vec![line], Vec::new())?;
            }
            Ok(NONE)
        }
        _ => {
            let mut hint = super::files::read_length(ctx, args.first().copied())?;
            let bounded = hint > 0;
            let iter = ctx.iterator(receiver)?;
            let mut result = Vec::new();
            while let Some(line) = ctx.next_item(iter)? {
                result
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                result.push(line);
                if bounded {
                    let len = ctx.builtin("len");
                    let length = ctx.call(len, vec![line], Vec::new())?;
                    hint -= ctx.as_integer(length)?;
                    if hint < 0 {
                        break;
                    }
                }
            }
            Ok(ctx.alloc(Value::List(result)))
        }
    }
}

/// Call an inherited I/O method with its bound receiver or an explicit class-level receiver.
pub(in super::super) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let bytes_io = ctx.builtins.get("_io.BytesIO").copied();
    let raw = ctx.builtins.get("_io._RawIOBase") == Some(&owner);
    let buffered = ctx.builtins.get("_io._BufferedIOBase") == Some(&owner);
    let descriptor = if raw {
        Some("_RawIOBase")
    } else if buffered {
        Some("_BufferedIOBase")
    } else if bytes_io == Some(owner) {
        Some("BytesIO")
    } else {
        None
    };
    let Some((&receiver, rest)) = args.split_first() else {
        let message = match (descriptor, name) {
            (Some("BytesIO"), "__init__") => {
                "descriptor '__init__' of '_io.BytesIO' object needs an argument".into()
            }
            (Some(class), _) => format!("unbound method {class}.{name}() needs an argument"),
            _ => format!("unbound I/O method {name} needs a receiver"),
        };
        return Err(Error::new("TypeError", message));
    };
    if !ctx.instance_of(receiver, owner) {
        let actual = ctx.type_name(receiver);
        let message = match (descriptor, name) {
            (Some("BytesIO"), "__init__") => format!(
                "descriptor '__init__' requires a '_io.BytesIO' object but received a '{actual}'"
            ),
            (Some(class), _) => format!(
                "descriptor '{name}' for '_io.{class}' objects doesn't apply to a '{actual}' object"
            ),
            _ => format!("invalid receiver for {name}"),
        };
        return Err(Error::new("TypeError", message));
    }
    if raw && matches!(name, "write" | "readinto") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("_RawIOBase.{name}() takes no keyword arguments"),
            ));
        }
        return Err(Error::new("NotImplementedError", ""));
    }
    if bytes_io != Some(owner)
        && matches!(name, "__iter__" | "__next__" | "readlines" | "writelines")
    {
        return lines(ctx, receiver, name, rest, kwargs);
    }
    if name == "__del__" {
        if !rest.is_empty() || !kwargs.is_empty() {
            return Err(Error::new("TypeError", "expected no arguments to __del__"));
        }
        // A detached or partially initialized stream can no longer be closed.
        let closed = ctx
            .lookup_attr(receiver, "closed")
            .and_then(|id| ctx.truth(id));
        if matches!(closed, Ok(false)) {
            let _ = ctx.set_attr(receiver, "_finalizing", TRUE);
            if let Err(error) = ctx
                .lookup_attr(receiver, "close")
                .and_then(|f| ctx.call(f, Vec::new(), Vec::new()))
            {
                eprintln!(
                    "Exception ignored while finalizing file {}: {error}",
                    ctx.repr(receiver)
                );
            }
        }
        return Ok(NONE);
    }
    if name == "detach" && (ctx.builtins.get("_io._TextIOBase") == Some(&owner) || buffered) {
        if !rest.is_empty() || !kwargs.is_empty() {
            return Err(Error::new("TypeError", "detach() takes no arguments"));
        }
        return Err(Error::new("UnsupportedOperation", "detach"));
    }
    if buffered {
        return buffered_io::call(ctx, receiver, name, rest, kwargs);
    }
    if ctx.builtins.get("_io._TextIOBase") == Some(&owner) {
        // The base class getters return None even on a closed or uninitialized subclass.
        return Ok(NONE);
    }
    if ctx
        .builtins
        .get("_io.TextIOWrapper")
        .is_some_and(|&class| ctx.instance_of(receiver, class))
    {
        if matches!(ctx.heap.get(receiver), Value::TextFile(_)) {
            return super::files::text_method(ctx, name, args, kwargs);
        }
        return text_wrapper::call(ctx, receiver, name, rest, kwargs);
    }
    if let Some(&file) = ctx.builtins.get("_io.FileIO")
        && (owner == file
            || matches!(name, "__enter__" | "__exit__" | "flush")
                && ctx.instance_of(receiver, file))
    {
        return file_io::call(ctx, receiver, name, rest, kwargs);
    }
    if ctx.builtins.get("_io.BufferedReader") == Some(&owner) {
        if let Value::File(handle) = ctx.heap.get(receiver) {
            if name == "closed" {
                return Ok(if handle.closed() { TRUE } else { FALSE });
            }
            return super::files::call(ctx, &format!("file.{name}"), args, kwargs);
        }
        return buffered_reader::call(ctx, receiver, name, rest, kwargs);
    }
    if bytes_io == Some(owner)
        || matches!(name, "__enter__" | "__exit__")
            && bytes_io.is_some_and(|class| ctx.instance_of(receiver, class))
    {
        return bytes_io::call(ctx, receiver, name, rest, kwargs);
    }
    if name == "__init__" {
        return string_io_init(ctx, receiver, rest, kwargs);
    }
    if ctx
        .builtins
        .get("_io.StringIO")
        .is_some_and(|&class| ctx.instance_of(receiver, class))
    {
        return string_io_method(ctx, receiver, name, rest, kwargs);
    }
    let arity = match name {
        "read" => 1..=2,
        "__exit__" => 4..=4,
        _ => 1..=1,
    };
    if !arity.contains(&args.len()) || !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    let Value::Instance { .. } = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an IOBase instance"));
    };
    let closed = ctx
        .raw_instance_field(receiver, "__IOBase_closed")
        .is_some();
    match name {
        "closed" => Ok(if closed { TRUE } else { FALSE }),
        "readable" | "writable" | "seekable" => Ok(FALSE),
        "close" if closed => Ok(NONE),
        "close" => {
            let result = ctx
                .lookup_attr(receiver, "flush")
                .and_then(|f| ctx.call(f, Vec::new(), Vec::new()));
            ctx.set_attr(receiver, "__IOBase_closed", TRUE)?;
            result.map(|_| NONE)
        }
        "flush" | "__enter__" if closed => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "flush" => Ok(NONE),
        "__enter__" => Ok(receiver),
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "read" => {
            let size = args
                .get(1)
                .map(|&id| ctx.as_integer(id))
                .transpose()?
                .unwrap_or(-1);
            if size < 0 {
                let readall = ctx.lookup_attr(receiver, "readall")?;
                return ctx.call(readall, Vec::new(), Vec::new());
            }
            let buffer = ctx.alloc(Value::ByteArray(zeroed_bytes(size)?));
            let readinto = ctx.lookup_attr(receiver, "readinto")?;
            let count = ctx.call(readinto, vec![buffer], Vec::new())?;
            if count == NONE {
                return Ok(NONE);
            }
            let count = ctx.as_integer(count)?;
            let Value::ByteArray(data) = ctx.heap.get(buffer) else {
                unreachable!()
            };
            if count < 0 || count > size || count as usize > data.len() {
                return Err(Error::new(
                    "ValueError",
                    format!("readinto returned {count} outside buffer size {size}"),
                ));
            }
            Ok(ctx.alloc(Value::Bytes(data[..count as usize].to_vec())))
        }
        "readall" => {
            let mut out = Vec::new();
            let size = ctx.int(DEFAULT_BUFFER_SIZE);
            loop {
                let read = ctx.lookup_attr(receiver, "read")?;
                let data = ctx.call(read, vec![size], Vec::new())?;
                if data == NONE {
                    if out.is_empty() {
                        return Ok(NONE);
                    }
                    break;
                }
                let Value::Bytes(data) = ctx.heap.get(data) else {
                    return Err(Error::new("TypeError", "read() should return bytes"));
                };
                if data.is_empty() {
                    break;
                }
                out.try_reserve(data.len())
                    .map_err(|_| Error::new("MemoryError", "read result is too large"))?;
                out.extend_from_slice(data);
            }
            Ok(ctx.alloc(Value::Bytes(out)))
        }
        _ => unreachable!("registered IOBase method"),
    }
}

/// Bind the C StringIO signature before installing the owned stream on the Python instance.
fn string_io_init(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 2 {
        return Err(Error::new(
            "TypeError",
            format!("StringIO() takes at most 2 arguments ({count} given)"),
        ));
    }
    let mut values = [None; 2];
    for (slot, arg) in values.iter_mut().zip(args) {
        *slot = Some(*arg);
    }
    for (key, value) in kwargs {
        let index = match key.as_str() {
            "initial_value" => 0,
            "newline" => 1,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("StringIO() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if values[index].replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "argument for StringIO() given by name ('{key}') and position ({})",
                    index + 1
                ),
            ));
        }
    }
    // The C newline parameter is converted to UTF-8, even when the initial value may
    // contain lone surrogates. Report the codec's actual exception for that parameter.
    let newline = match values[1] {
        None => Some("\n"),
        Some(NONE) => None,
        Some(id) => match ctx.heap.get(ctx.scalar_value(id)) {
            Value::Str(value) => Some(value.as_str()),
            Value::SurrogateStr(_) => {
                ctx.encode_utf8_codepoints(id, false, "strict")?;
                unreachable!("a surrogate cannot be encoded in strict UTF-8")
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("newline must be str or None, not {}", ctx.type_name(id)),
                ));
            }
        },
    };
    let file = crate::objects::TextFile::string_io(None, newline)?;
    if let Some(initial) = values[0].filter(|&id| id != NONE) {
        match ctx.heap.get(ctx.scalar_value(initial)) {
            Value::Str(text) => file.write(text)?,
            Value::SurrogateStr(points) => file.write_memory(points)?,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "initial_value must be str or None, not {}",
                        ctx.type_name(initial)
                    ),
                ));
            }
        }
        file.seek_memory(0, 0)?;
    }
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("StringIO.__init__ has an instance receiver");
    };
    *memory_stream = Some(MemoryStream::Text(file));
    Ok(NONE)
}

fn newline_history(ctx: &mut Context, seen: u8) -> Id {
    let mut found = Vec::new();
    for (bit, spelling) in [(1, "\r"), (2, "\n"), (4, "\r\n")] {
        if seen & bit != 0 {
            found.push(ctx.text(spelling));
        }
    }
    match found.as_slice() {
        [] => NONE,
        [id] => *id,
        _ => ctx.alloc(Value::Tuple(found)),
    }
}

impl From<crate::objects::MemoryText> for Value {
    fn from(snapshot: crate::objects::MemoryText) -> Self {
        match snapshot {
            crate::objects::MemoryText::Utf8(text) => Value::Str(text.into_string()),
            crate::objects::MemoryText::Surrogates(points) => Value::SurrogateStr(points),
        }
    }
}

fn string_io_file(ctx: &Context, receiver: Id) -> Result<&crate::objects::TextFile> {
    let Value::Instance { memory_stream, .. } = ctx.heap.get(receiver) else {
        unreachable!("StringIO methods have an instance receiver");
    };
    match memory_stream {
        Some(MemoryStream::Text(file)) => Ok(file),
        _ => Err(Error::new(
            "ValueError",
            "I/O operation on uninitialized object",
        )),
    }
}

fn string_io_method(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("StringIO.{name}() takes no keyword arguments"),
        ));
    }
    argument_count("StringIO", name, args.len())?;
    if name == "__exit__" {
        let close = ctx.lookup_attr(receiver, "close")?;
        return ctx.call(close, Vec::new(), Vec::new());
    }
    // An index hook can close or reinitialize the receiver. Fetch the live file after conversion.
    if matches!(name, "read" | "readline" | "truncate") {
        let size = args
            .first()
            .copied()
            .filter(|&id| id != NONE)
            .map(|id| super::files::read_length(ctx, Some(id)))
            .transpose()?;
        let file = string_io_file(ctx, receiver)?;
        if name == "truncate" {
            let n = file.truncate_memory(size.map(|n| n as isize))?;
            return Ok(ctx.int(n as i64));
        }
        let limit = size.and_then(|n| usize::try_from(n).ok());
        let text = file.read_memory(limit, name == "readline")?;
        return Ok(ctx.alloc(text.into()));
    }
    if name == "seek" {
        let pos = ctx.index_integer(args[0])?.to_i64().ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
        let whence = if let Some(&id) = args.get(1) {
            ctx.index_integer(id)?
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?
        } else {
            0
        };
        let n = string_io_file(ctx, receiver)?.seek_memory(pos, whence)?;
        return Ok(ctx.int(n as i64));
    }
    let file = match string_io_file(ctx, receiver) {
        Ok(file) => file,
        Err(_) if matches!(name, "close" | "flush") => return Ok(NONE),
        Err(error) => return Err(error),
    };
    match name {
        "write" => {
            let count = match ctx.heap.get(ctx.scalar_value(args[0])) {
                Value::Str(text) => {
                    file.write(text)?;
                    text.chars().count()
                }
                Value::SurrogateStr(points) => {
                    file.write_memory(points)?;
                    points.len()
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("string argument expected, got '{}'", ctx.type_name(args[0])),
                    ));
                }
            };
            Ok(ctx.int(count as i64))
        }
        "getvalue" => Ok(ctx.alloc(file.memory_value()?.into())),
        "tell" => Ok(ctx.int(file.memory_position()? as i64)),
        "newlines" => {
            let seen = file.memory_newlines()?;
            Ok(newline_history(ctx, seen))
        }
        "closed" => Ok(if file.closed() { TRUE } else { FALSE }),
        "flush" => {
            file.flush()?;
            Ok(NONE)
        }
        "close" => {
            file.close()?;
            Ok(NONE)
        }
        "__enter__" | "isatty" if file.closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "__enter__" => Ok(receiver),
        "isatty" => Ok(FALSE),
        "readable" | "writable" | "seekable" | "line_buffering" if file.closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file"))
        }
        "line_buffering" => Ok(FALSE),
        "readable" | "writable" | "seekable" => Ok(TRUE),
        _ => unreachable!("registered StringIO method"),
    }
}
