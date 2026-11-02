//! Native I/O classes for runtime-loaded modules.

use super::super::builtins::zeroed_bytes;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

mod newline_decoder;
pub(in crate::dynamic) const NEWLINE_DECODER: &str = "IncrementalNewlineDecoder";
mod buffered_io;
mod descriptor_buffer;
mod native_reader;
mod native_writer;
pub(in crate::dynamic) use descriptor_buffer::{
    BufferedFd, note_writer_finalizer, prepare_writer_shutdown,
};
mod bytes_io;
mod bytes_writer;
mod docstrings;
mod file_io;
pub(super) use file_io::{
    check_binary_options, check_open_string, check_text_options, mode as fileio_mode,
    validate_open_mode,
};
pub(in crate::dynamic) use file_io::{open, popen_pipe};
mod text_wrapper;
pub(super) use text_wrapper::text_options;

#[derive(Clone)]
pub(in crate::dynamic) enum MemoryStream {
    Text(crate::objects::TextFile),
    BufferedFile(Id),
    BufferedBytes(std::rc::Rc<bytes_writer::Writer>),
    Bytes(std::rc::Rc<bytes_io::BytesIo>),
    Wrapper(std::rc::Rc<text_wrapper::Wrapper>),
    Raw(std::rc::Rc<file_io::RawFile>),
    Decoder(newline_decoder::Decoder),
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

/// Build the interpreter's shared text reader without taking ownership of fd 0.
pub(in crate::dynamic) fn standard_input(ctx: &mut Context) -> Result<Id> {
    let fd = ctx.int(0);
    let mode = ctx.text("r");
    let buffering = ctx.int(-1);
    let encoding = ctx.text(crate::sys::standard_field(0, false).as_str());
    let errors = ctx.text(crate::sys::standard_field(0, true).as_str());
    let newline = ctx.text("\n");
    let stream = match open(
        ctx,
        &[fd, mode, buffering, encoding, errors, newline, FALSE],
        &[],
    ) {
        Err(error)
            if matches!(error.os_args.as_deref(),
                Some(super::super::os_error::Args::Native { source, .. }) if source.errno == i64::from(libc::EBADF)
            ) =>
        {
            return Ok(NONE);
        }
        result => result?,
    };
    let buffer = ctx.lookup_attr(stream, "buffer")?;
    let raw = ctx.lookup_attr(buffer, "raw")?;
    let name = ctx.text("<stdin>");
    ctx.set_attr(raw, "name", name)?;
    Ok(stream)
}

/// Follow strong edges from the original sys module without retaining any of the streams.
pub(in crate::dynamic) fn rooted_input_chain(ctx: &Context) -> Option<[Id; 3]> {
    let Value::Module { scope, .. } = *ctx.heap.get(ctx.threads.system_module?) else {
        return None;
    };
    let stdin = ctx.scope_name(scope, "__stdin__")?;
    let Value::Instance {
        memory_stream: Some(MemoryStream::Wrapper(wrapper)),
        ..
    } = ctx.heap.get(stdin)
    else {
        return None;
    };
    let buffer = wrapper.buffer.get()?;
    let Value::File(file) = ctx.heap.get(buffer) else {
        return None;
    };
    let super::BinaryFile::Descriptor(fd) = file.as_ref() else {
        return None;
    };
    Some([stdin, buffer, fd.raw])
}

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

/// Install public metadata and the types of native buffered file handles.
pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let names = PUBLIC_NAMES
        .iter()
        .map(|name| ctx.heap.code_name(name))
        .collect();
    let exports = ctx.alloc(Value::List(names));
    ctx.bind(module, "__all__", exports);
    let open_code = ctx.builtin("io.open_code");
    ctx.bind(module, "open_code", open_code);
    for (name, origin) in [("SEEK_SET", 0), ("SEEK_CUR", 1), ("SEEK_END", 2)] {
        let value = ctx.int(origin);
        ctx.bind(module, name, value);
    }
    let buffered_random = ctx.builtin("_io.BufferedRandom");
    ctx.bind(module, "_buffered_random", buffered_random);
}

/// Construct a low-level module's native I/O classes before io.py declares its ABCs.
pub(in super::super) fn install_native(ctx: &mut Context, module: Id, module_object: Id) {
    let text_encoding = ctx.builtin("_io.text_encoding");
    ctx.bind(module, "text_encoding", text_encoding);
    let unsupported = ctx.builtin("UnsupportedOperation");
    ctx.bind(module, "UnsupportedOperation", unsupported);
    let size = ctx.int(DEFAULT_BUFFER_SIZE);
    ctx.bind(module, "DEFAULT_BUFFER_SIZE", size);
    let object = ctx.builtin("object");
    class(
        ctx,
        module,
        NEWLINE_DECODER,
        object,
        newline_decoder::METHODS,
    );
    let io = class(
        ctx,
        module,
        "_IOBase",
        object,
        &[
            "closed",
            "__del__",
            "__iter__",
            "__next__",
            "readline",
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
        "_RawIOBase",
        io,
        &["read", "readall", "readinto", "write"],
    );
    let file = class(ctx, module, "FileIO", raw, file_io::METHODS);
    let buffered = class(ctx, module, "_BufferedIOBase", io, buffered_io::METHODS);
    let reader = class(
        ctx,
        module,
        "BufferedReader",
        buffered,
        native_reader::METHODS,
    );
    let writer = class(
        ctx,
        module,
        "BufferedWriter",
        buffered,
        native_writer::METHODS,
    );
    let bytes_io = class(ctx, module, "BytesIO", buffered, bytes_io::METHODS);
    let text = class(
        ctx,
        module,
        "_TextIOBase",
        io,
        &[
            "encoding", "errors", "newlines", "detach", "read", "readline", "write",
        ],
    );
    let wrapper = class(ctx, module, "TextIOWrapper", text, text_wrapper::METHODS);
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
    // Native open and intrinsic file objects retain their initial _io classes.
    let initial = !ctx.builtins.contains_key("_io._IOBase");
    let opener = if initial {
        ctx.builtins
            .insert("_io.open.__self__".into(), module_object);
        ctx.builtin("open")
    } else {
        let callable = ctx.alloc(Value::IoOpen { reader, writer });
        ctx.alloc(Value::Method {
            callable,
            receiver: module_object,
            explicit: false,
        })
    };
    ctx.bind(module, "open", opener);
    if initial {
        for (name, class) in [
            ("_io._IOBase", io),
            ("_io._RawIOBase", raw),
            ("_io._BufferedIOBase", buffered),
            ("_io._TextIOBase", text),
            ("_io.BufferedReader", reader),
            ("_io.BufferedWriter", writer),
            ("_io.FileIO", file),
            ("_io.BytesIO", bytes_io),
            ("_io.TextIOWrapper", wrapper),
            ("_io.StringIO", string_io),
        ] {
            ctx.builtins.insert(name.into(), class);
        }
    }
}

/// The native classes are immutable; Python I/O ABCs and user subclasses remain mutable.
pub(in super::super) fn check_class_write(ctx: &Context, object: Id, attr: &str) -> Result<()> {
    if let Value::Class { io_kind: Some(_), name, .. } = ctx.heap.get(object) {
        return Err(Error::new(
            "TypeError",
            format!("cannot set '{attr}' attribute of immutable type '_io.{name}'"),
        ));
    }
    Ok(())
}

fn class(
    ctx: &mut Context,
    module: Id,
    name: &'static str,
    base: Id,
    methods: &[&'static str],
) -> Id {
    let scope = ctx.scope("_io", Some(module));
    let owner = ctx
        .allocate_class(name.into(), scope, vec![base])
        .expect("a single I/O base has a linearization");
    let Value::Class { io_kind, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *io_kind = Some(name.trim_start_matches('_'));
    if name == NEWLINE_DECODER
        && let Value::Scope(frame) = ctx.heap.get_mut(scope)
    {
        frame.class_namespace.as_mut().unwrap().slot_names = Some([].into());
    }
    if let Some(doc) = docstrings::base(name) {
        let doc = ctx.text(doc);
        ctx.bind(scope, "__doc__", doc);
    }
    let module_name = ctx.text("_io");
    ctx.bind(scope, "__module__", module_name);
    let qualname = ctx.text(name);
    ctx.bind(scope, "__qualname__", qualname);
    let wrapper = name == "TextIOWrapper";
    let descriptor_owner = match name {
        "_TextIOBase" => "_io._TextIOBase",
        "StringIO" => "_io.StringIO",
        "FileIO" => "_io.FileIO",
        "TextIOWrapper" => "_io.TextIOWrapper",
        "BufferedReader" => "_io.BufferedReader",
        "BufferedWriter" => "_io.BufferedWriter",
        "BytesIO" => "_io.BytesIO",
        NEWLINE_DECODER => newline_decoder::CLASS,
        _ => "_io._IOBase",
    };
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod { owner, name });
        let attr = if matches!(
            name,
            "closed" | "encoding" | "errors" | "line_buffering" | "newlines"
        ) || descriptor_owner == "_io.FileIO"
            && matches!(name, "mode" | "closefd" | "_blksize")
            || matches!(
                descriptor_owner,
                "_io.BufferedReader" | "_io.BufferedWriter"
            ) && matches!(name, "raw" | "mode" | "name")
            || wrapper && text_wrapper::property(name)
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

/// Text and buffered bases validate their C signatures before rejecting a transfer.
fn unsupported_transfer(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    text: bool,
) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 1 {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes at most 1 argument ({count} given)"),
        ));
    }
    if name == "write" {
        let Some(&value) = args.first() else {
            return Err(Error::new(
                "TypeError",
                "write() takes exactly 1 positional argument (0 given)",
            ));
        };
        if text {
            let nul = match ctx.heap.get(ctx.scalar_value(value)) {
                Value::Str(s) => s.contains('\0'),
                Value::SurrogateStr(_) => ctx
                    .encode_utf8_codepoints(value, false, "strict")?
                    .contains(&0),
                _ => {
                    let kind = if value == NONE {
                        "None"
                    } else {
                        ctx.type_name(value)
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!("write() argument 1 must be str, not {kind}"),
                    ));
                }
            };
            if nul {
                return Err(Error::new("ValueError", "embedded null character"));
            }
        }
    } else if let Some((key, _)) = kwargs.first() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() got an unexpected keyword argument '{key}'"),
        ));
    } else if let Some(&size) = args.first() {
        ctx.index_integer(size)?
            .to_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })?;
    }
    Err(Error::new("UnsupportedOperation", name))
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

/// IOBase.readline uses a cached optional peek method and resolves read for every chunk.
fn readline(ctx: &mut Context, receiver: Id, size: Option<Id>) -> Result<Id> {
    fn bytes<'a>(ctx: &'a Context, result: Id, method: &str) -> Result<&'a [u8]> {
        match ctx.heap.get(ctx.scalar_value(result)) {
            Value::Bytes(data) => Ok(data),
            _ => Err(Error::new(
                "OSError",
                format!(
                    "{method}() should have returned a bytes object, not '{}'",
                    ctx.type_name(result)
                ),
            )),
        }
    }

    let limit = usize::try_from(super::files::read_length(ctx, size)?).unwrap_or(usize::MAX);
    let peek = match ctx.lookup_attr(receiver, "peek") {
        Ok(method) => Some(method),
        Err(error) if ctx.is_attribute_error(&error) => None,
        Err(error) => return Err(error),
    };
    if let Some(method) = peek {
        ctx.heap.keep(method);
    }
    let step = ctx.heap.temporary_mark();
    let mut line = Vec::new();
    while line.len() < limit {
        ctx.heap.release_temporaries_since(step);
        let mut size = 1;
        if let Some(peek) = peek {
            let one = ctx.int(1);
            let result = match ctx.call(peek, vec![one], Vec::new()) {
                Err(error) if ctx.has_errno(&error, libc::EINTR) => continue,
                result => result?,
            };
            let ahead = bytes(ctx, result, "peek")?;
            let ahead = &ahead[..ahead.len().min(limit)];
            if !ahead.is_empty() {
                size = ahead
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(ahead.len(), |i| i + 1);
            }
        }
        let count = ctx.int(size as i64);
        let result = match ctx
            .lookup_attr(receiver, "read")
            .and_then(|read| ctx.call(read, vec![count], Vec::new()))
        {
            Err(error) if ctx.has_errno(&error, libc::EINTR) => continue,
            result => result?,
        };
        let data = bytes(ctx, result, "read")?;
        line.try_reserve(data.len())
            .map_err(|_| crate::Error::allocation())?;
        line.extend_from_slice(data);
        if data.is_empty() || data.ends_with(b"\n") {
            break;
        }
    }
    Ok(ctx.alloc(Value::Bytes(line)))
}

/// IOBase consumes the receiver's actual iterator and each writer override.
pub(super) fn lines(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (owner, name) = if let Some(method) = name.strip_prefix("file.") {
        (ctx.type_name(receiver).trim_start_matches("_io."), method)
    } else {
        ("_IOBase", name)
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    argument_count(owner, name, args.len())?;
    if matches!(name, "__iter__" | "writelines") {
        let closed = ctx.lookup_attr(receiver, "closed")?;
        if ctx.truth(closed)? {
            return Err(Error::new("ValueError", "I/O operation on closed file."));
        }
    }
    match name {
        "readline" => readline(ctx, receiver, args.first().copied()),
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
            ctx.heap.keep(iterator);
            let step = ctx.heap.temporary_mark();
            while let Some(line) = ctx.next_item(iterator)? {
                ctx.heap.keep(line);
                let write = ctx.lookup_attr(receiver, "write")?;
                ctx.call(write, vec![line], Vec::new())?;
                ctx.heap.release_temporaries_since(step);
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

/// Open a file using the native buffered classes captured by a fresh low-level module.
pub(in super::super) fn open_from_module(
    ctx: &mut Context,
    reader: Id,
    writer: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((_, arguments)) = args.split_first() else {
        return Err(Error::new("TypeError", "_io.open needs its module"));
    };
    let file = open(ctx, arguments, kwargs)?;
    let class = match ctx.heap.get(file).native_io_class_name() {
        Some("_io.BufferedReader") => reader,
        Some("_io.BufferedWriter") => writer,
        _ => return Ok(file),
    };
    let stream = ctx.instance(class, None);
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(stream) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::BufferedFile(file));
    Ok(stream)
}

/// Call an inherited I/O method with its bound receiver or an explicit class-level receiver.
pub(in super::super) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let owner_kind = ctx.heap.get(owner).io_kind();
    if owner_kind == Some(NEWLINE_DECODER) {
        return newline_decoder::call(ctx, owner, name, args, kwargs);
    }
    let raw = owner_kind == Some("RawIOBase");
    let buffered = owner_kind == Some("BufferedIOBase");
    let text = owner_kind == Some("TextIOBase");
    let bytes_io = owner_kind == Some("BytesIO");
    let descriptor = if raw {
        Some("_RawIOBase")
    } else if buffered {
        Some("_BufferedIOBase")
    } else if text {
        Some("_TextIOBase")
    } else if matches!(owner_kind, Some("BufferedReader" | "BufferedWriter")) {
        owner_kind
    } else if owner_kind == Some("TextIOWrapper") && name == "__next__" {
        Some("TextIOWrapper")
    } else if bytes_io {
        Some("BytesIO")
    } else {
        None
    };
    let Some((&receiver, rest)) = args.split_first() else {
        let message = match (descriptor, name) {
            (Some(class @ ("BytesIO" | "BufferedReader" | "BufferedWriter")), "__init__") => {
                format!("descriptor '__init__' of '_io.{class}' object needs an argument")
            }
            (Some("TextIOWrapper"), "__next__") => {
                "descriptor '__next__' of '_io.TextIOWrapper' object needs an argument".into()
            }
            (Some(class), _) => format!("unbound method {class}.{name}() needs an argument"),
            _ => format!("unbound I/O method {name} needs a receiver"),
        };
        return Err(Error::new("TypeError", message));
    };
    let receiver_class = match ctx.heap.get(receiver) {
        Value::Instance { class, .. } => Some(*class),
        value => value
            .native_io_class_name()
            .and_then(|name| ctx.builtins.get(name).copied()),
    };
    let Some(receiver_class) = receiver_class.filter(|&class| ctx.direct_subclass(class, owner))
    else {
        let actual = ctx.type_name(receiver);
        let message = match (descriptor, name) {
            (Some(class @ ("BytesIO" | "BufferedReader" | "BufferedWriter")), "__init__") => {
                format!(
                    "descriptor '__init__' requires a '_io.{class}' object but received a '{actual}'"
                )
            }
            (Some("TextIOWrapper"), "__next__") => {
                let actual = receiver_class
                    .and_then(|class| ctx.heap.get(class).io_kind())
                    .map_or_else(|| actual.to_string(), |_| format!("_io.{actual}"));
                format!(
                    "descriptor '__next__' requires a '_io.TextIOWrapper' object but received a '{actual}'"
                )
            }
            (Some(class), _) => format!(
                "descriptor '{name}' for '_io.{class}' objects doesn't apply to a '{actual}' object"
            ),
            _ => format!("invalid receiver for {name}"),
        };
        return Err(Error::new("TypeError", message));
    };
    match owner_kind {
        Some("TextIOWrapper")
            if name == "__next__" && matches!(ctx.heap.get(receiver), Value::Instance { .. }) =>
        {
            return text_wrapper::next(ctx, receiver, receiver_class == owner, rest, kwargs);
        }
        Some("BufferedReader") => return native_reader::call(ctx, receiver, name, rest, kwargs),
        Some("BufferedWriter") => return native_writer::call(ctx, receiver, name, rest, kwargs),
        _ => {}
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
    if owner_kind == Some("IOBase") && name == "readline"
        || !bytes_io && matches!(name, "__iter__" | "__next__" | "readlines" | "writelines")
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
    if name == "detach" && (text || buffered) {
        if !rest.is_empty() || !kwargs.is_empty() {
            return Err(Error::new("TypeError", "detach() takes no arguments"));
        }
        return Err(Error::new("UnsupportedOperation", "detach"));
    }
    if (text || buffered) && matches!(name, "write" | "read" | "read1" | "readline") {
        return unsupported_transfer(ctx, name, rest, kwargs, text);
    }
    if buffered {
        return buffered_io::call(ctx, receiver, name, rest, kwargs);
    }
    if text {
        // The base class getters return None even on a closed or uninitialized subclass.
        return Ok(NONE);
    }
    let Value::Class { ancestors, .. } = ctx.heap.get(receiver_class) else {
        unreachable!()
    };
    let layout = std::iter::once(&receiver_class)
        .chain(ancestors)
        .filter_map(|&class| ctx.heap.get(class).io_kind())
        .find(|kind| {
            matches!(
                *kind,
                "TextIOWrapper"
                    | "FileIO"
                    | "BytesIO"
                    | "StringIO"
                    | "BufferedReader"
                    | "BufferedWriter"
            )
        });
    if layout == Some("BufferedReader") && matches!(name, "__enter__" | "__exit__" | "writable") {
        return native_reader::call(ctx, receiver, name, rest, kwargs);
    }
    if layout == Some("BufferedWriter") && matches!(name, "__enter__" | "__exit__" | "readable") {
        return native_writer::call(ctx, receiver, name, rest, kwargs);
    }
    if layout == Some("TextIOWrapper") {
        if matches!(ctx.heap.get(receiver), Value::TextFile(_)) {
            return super::files::text_method(ctx, name, args, kwargs);
        }
        return text_wrapper::call(ctx, receiver, name, rest, kwargs);
    }
    if owner_kind == Some("FileIO")
        || matches!(name, "__enter__" | "__exit__" | "flush") && layout == Some("FileIO")
    {
        return file_io::call(ctx, receiver, name, rest, kwargs);
    }
    if bytes_io || matches!(name, "__enter__" | "__exit__") && layout == Some("BytesIO") {
        return bytes_io::call(ctx, receiver, name, rest, kwargs);
    }
    if name == "__init__" {
        return string_io_init(ctx, receiver, rest, kwargs);
    }
    if layout == Some("StringIO") {
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
