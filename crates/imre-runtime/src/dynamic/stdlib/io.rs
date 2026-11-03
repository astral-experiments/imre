//! Native I/O classes for runtime-loaded modules.

use super::super::{Heap, builtins::zeroed_bytes, value::IoOpenTypes};
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

/// Release one native owner's strong edge before the next observable state change.
fn release_child(ctx: &mut Context, child: Option<Id>) -> Result<()> {
    if let Some(child) = child {
        let (heap, roots) = ctx.heap_and_roots();
        if heap.released_last_finalizer_ref(child, roots)? {
            ctx.dispatch_finalizer(child);
        }
    }
    Ok(())
}

mod newline_decoder;
pub(in crate::dynamic) const NEWLINE_DECODER: &str = "IncrementalNewlineDecoder";
mod buffered_io;
mod buffered_reader;
mod descriptor_buffer;
mod native_random;
mod native_reader;
mod native_writer;
mod rwpair;
pub(in crate::dynamic) use descriptor_buffer::{
    BufferedFd, note_writer_finalizer, prepare_writer_shutdown,
};
pub(in crate::dynamic) use rwpair::deallocate_exact_pair;
mod bytes_io;
mod bytes_writer;
pub(in crate::dynamic) use bytes_writer::pending_callback_write;
mod docstrings;
mod file_io;
pub(super) use file_io::{
    check_binary_options, check_open_string, check_text_options, mode as fileio_mode,
    validate_open_mode,
};
pub(in crate::dynamic) use file_io::{open, popen_pipe};
mod text_wrapper;

#[derive(Clone, Copy)]
pub(in crate::dynamic) enum NativeClass {
    Named(&'static str),
    Pair { reader: Id, writer: Id },
}

#[derive(Clone)]
pub(in crate::dynamic) enum MemoryStream {
    Text(crate::objects::TextFile),
    BufferedFile(Id),
    Reader(std::rc::Rc<buffered_reader::Reader>),
    UninitializedReader(Option<Id>),
    UninitializedWriter(Option<Id>),
    DetachedWriter,
    BufferedBytes(std::rc::Rc<bytes_writer::Writer>),
    Pair(rwpair::Pair),
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

/// An absent standard descriptor is exposed as None in both sys bindings.
pub(in crate::dynamic) fn standard_output(ctx: &mut Context, stderr: bool) -> Result<Id> {
    let fd = if stderr {
        libc::STDERR_FILENO
    } else {
        libc::STDOUT_FILENO
    };
    // The flag query uses F_GETFD, which also checks whether this descriptor exists.
    if crate::objects::os::INHERITABLE.enabled(fd).is_err() {
        return Ok(NONE);
    }
    if ctx.config["unbuffered"].as_bool() == Some(true) {
        return unbuffered_standard(ctx, fd, if stderr { "<stderr>" } else { "<stdout>" });
    }
    let file = crate::objects::TextFile::standard(stderr);
    let path = ctx.text(if stderr { "<stderr>" } else { "<stdout>" });
    let buffer = ctx.alloc(
        super::BinaryFile::Standard {
            file: file.clone(),
            path,
        }
        .into_value()?,
    );
    Ok(
        ctx.alloc(Value::TextFile(super::super::value::BoxedTextFile {
            file,
            buffer: Some(buffer),
            finalizing: false,
            line_buffering: stderr || crate::stdio_settings().stdout_line_buffered,
        })),
    )
}

/// Build the interpreter's shared text reader without taking ownership of fd 0.
pub(in crate::dynamic) fn standard_input(ctx: &mut Context) -> Result<Id> {
    if ctx.config["unbuffered"].as_bool() == Some(true) {
        if crate::objects::os::INHERITABLE.enabled(0).is_err() {
            return Ok(NONE);
        }
        return unbuffered_standard(ctx, 0, "<stdin>");
    }
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
        None,
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

/// Under -u the binary input remains buffered, while outputs wrap FileIO directly.
fn unbuffered_standard(ctx: &mut Context, fd: i32, name: &str) -> Result<Id> {
    let input = fd == 0;
    let descriptor = ctx.int(i64::from(fd));
    let mode = ctx.text(if input { "rb" } else { "wb" });
    let buffering = ctx.int(if input { -1 } else { 0 });
    let buffer = open(
        ctx,
        &[descriptor, mode, buffering, NONE, NONE, NONE, FALSE],
        &[],
        None,
    )?;
    let raw = if input {
        ctx.lookup_attr(buffer, "raw")?
    } else {
        buffer
    };
    let name = ctx.text(name);
    ctx.set_attr(raw, "name", name)?;
    let wrapper = ctx.builtins["_io.TextIOWrapper"];
    let encoding = ctx.text(crate::sys::standard_field(fd as u32, false).as_str());
    let errors = ctx.text(crate::sys::standard_field(fd as u32, true).as_str());
    let newline = ctx.text("\n");
    let stream = ctx.call(
        wrapper,
        vec![buffer, encoding, errors, newline, FALSE, TRUE],
        Vec::new(),
    )?;
    let mode = ctx.text(if input { "r" } else { "w" });
    ctx.set_attr(stream, "mode", mode)?;
    Ok(stream)
}

/// Follow strong edges from the original sys module without retaining any of the streams.
pub(in crate::dynamic) fn rooted_input_chain(ctx: &Context) -> Option<[Id; 3]> {
    let Value::Module { scope, .. } = *ctx.heap.get(ctx.threads.system_module?) else {
        return None;
    };
    text_descriptor_chain(&ctx.heap, ctx.scope_name(scope, "__stdin__")?)
}

/// Follow the stored strong references from a text wrapper to its buffer and raw file.
pub(in crate::dynamic) fn text_descriptor_chain(heap: &Heap, stream: Id) -> Option<[Id; 3]> {
    let Value::Instance {
        memory_stream,
        fields,
        ..
    } = heap.get(stream)
    else {
        return None;
    };
    let Some(MemoryStream::Wrapper(wrapper)) = memory_stream else {
        // _pyio keeps both links in instance fields. Once they move to a
        // materialized __dict__, leave them to the ordinary reachability check.
        let buffer = *fields.get("_buffer")?;
        let Value::Instance { fields, .. } = heap.get(buffer) else {
            return None;
        };
        return Some([stream, buffer, *fields.get("_raw")?]);
    };
    let buffer = wrapper.buffer.get()?;
    let Value::File(file) = heap.get(buffer) else {
        return None;
    };
    let super::BinaryFile::Descriptor(fd) = file.as_ref() else {
        return None;
    };
    Some([stream, buffer, fd.raw])
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
}

/// Qualify a native I/O failure at its originating call, before a Python override can forward it.
pub(in super::super) fn qualify_error(
    ctx: &mut Context,
    mut error: Error,
    class: Option<Id>,
) -> Error {
    if error.kind == "UnsupportedOperation" && error.class().is_none() {
        error.class = class.unwrap_or_else(|| ctx.builtin("UnsupportedOperation"));
    }
    error
}

/// Each low-level I/O module owns a mutable exception type with CPython's two bases.
pub(in super::super) fn unsupported_operation(ctx: &mut Context) -> Id {
    let scope = ctx.scope("io", None);
    let module = ctx.text("io");
    let name = ctx.text("UnsupportedOperation");
    ctx.bind(scope, "__module__", module);
    ctx.bind(scope, "__qualname__", name);
    ctx.bind(scope, "__doc__", NONE);
    let os_error = ctx.builtin("OSError");
    let value_error = ctx.builtin("ValueError");
    let class = ctx
        .allocate_class(
            "UnsupportedOperation".into(),
            scope,
            vec![os_error, value_error],
        )
        .expect("the standard I/O exception bases have a linearization");
    ctx.install_weakref_descriptor(class, scope);
    class
}

/// Construct a low-level module's native I/O classes before io.py declares its ABCs.
pub(in super::super) fn install_native(ctx: &mut Context, module: Id, module_object: Id) {
    let text_encoding = ctx.builtin("_io.text_encoding");
    ctx.bind(module, "text_encoding", text_encoding);
    // Intrinsic file objects retain the initial module's exception and stream classes.
    let initial = !ctx.builtins.contains_key("_io._IOBase");
    let unsupported = if initial {
        ctx.builtin("UnsupportedOperation")
    } else {
        unsupported_operation(ctx)
    };
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
    let pair = class(ctx, module, "BufferedRWPair", buffered, rwpair::METHODS);
    let Value::Class { io_kind, .. } = ctx.heap.get_mut(pair) else {
        unreachable!()
    };
    *io_kind = Some(NativeClass::Pair { reader, writer });
    let random = class(
        ctx,
        module,
        "BufferedRandom",
        buffered,
        native_random::METHODS,
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
            "__new__",
            "__init__",
            "__getstate__",
            "__setstate__",
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
    let opener = if initial {
        ctx.builtins
            .insert("_io.open.__self__".into(), module_object);
        ctx.builtin("open")
    } else {
        let callable = ctx.alloc(Value::IoOpen {
            types: IoOpenTypes {
                file,
                reader,
                writer,
                random,
                wrapper,
            },
            unsupported,
        });
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
            ("_io.BufferedRandom", random),
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
    if let Value::Class {
        io_kind: Some(_),
        name,
        ..
    } = ctx.heap.get(object)
    {
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
    *io_kind = Some(NativeClass::Named(name.trim_start_matches('_')));
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
        "BufferedRandom" => "_io.BufferedRandom",
        "BufferedReader" => "_io.BufferedReader",
        "BufferedWriter" => "_io.BufferedWriter",
        "BufferedRWPair" => "_io.BufferedRWPair",
        "BytesIO" => "_io.BytesIO",
        NEWLINE_DECODER => newline_decoder::CLASS,
        _ => "_io._IOBase",
    };
    if wrapper {
        let name = ctx.text("_finalizing");
        let member = ctx.alloc(Value::SlotMember { owner, name });
        ctx.bind(scope, "_finalizing", member);
    }
    let unsupported = ctx.scope_name(module, "UnsupportedOperation").unwrap();
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod {
            owner,
            name,
            unsupported,
        });
        let attr = if name == "__new__" {
            ctx.alloc(Value::Method {
                callable: method,
                receiver: owner,
                explicit: false,
            })
        } else if matches!(
            name,
            "closed" | "encoding" | "errors" | "line_buffering" | "newlines"
        ) || descriptor_owner == "_io.FileIO"
            && matches!(name, "mode" | "closefd" | "_blksize")
            || matches!(
                descriptor_owner,
                "_io.BufferedReader" | "_io.BufferedWriter" | "_io.BufferedRandom"
            ) && matches!(name, "raw" | "mode" | "name")
            || wrapper && text_wrapper::property(name)
        {
            let set = (wrapper && name == "_CHUNK_SIZE").then(|| {
                ctx.alloc(Value::IoMethod {
                    owner,
                    name: "set_chunk_size",
                    unsupported,
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
        "read" | "read1" | "readline" | "readlines" | "peek" | "truncate" if n > 1 => {
            format!("{name} expected at most 1 argument, got {n}")
        }
        "seek" if n == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if n > 2 => format!("seek expected at most 2 arguments, got {n}"),
        "write" | "readinto" | "writelines" | "__setstate__" if n != 1 => {
            format!("{owner}.{name}() takes exactly one argument ({n} given)")
        }
        "read" | "read1" | "readline" | "readlines" | "peek" | "writelines" | "seek" | "write"
        | "readinto" | "truncate" | "__exit__" | "__setstate__" => {
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
    } else if matches!(
        owner_kind,
        Some("BufferedReader" | "BufferedWriter" | "BufferedRWPair" | "BufferedRandom")
    ) {
        owner_kind
    } else if owner_kind == Some("TextIOWrapper")
        && matches!(name, "__next__" | "__repr__" | "truncate")
    {
        Some("TextIOWrapper")
    } else if bytes_io {
        Some("BytesIO")
    } else {
        None
    };
    let Some((&receiver, rest)) = args.split_first() else {
        let message = match (descriptor, name) {
            (
                Some(
                    class @ ("BytesIO" | "BufferedReader" | "BufferedWriter" | "BufferedRWPair"
                    | "BufferedRandom"),
                ),
                "__init__",
            ) => {
                format!("descriptor '__init__' of '_io.{class}' object needs an argument")
            }
            (Some("TextIOWrapper"), name @ ("__next__" | "__repr__")) => {
                format!("descriptor '{name}' of '_io.TextIOWrapper' object needs an argument")
            }
            (Some(class), _) => format!("unbound method {class}.{name}() needs an argument"),
            _ => format!("unbound I/O method {name} needs a receiver"),
        };
        return Err(Error::new("TypeError", message));
    };
    if let Some(kind @ ("BytesIO" | "StringIO")) = owner_kind
        && name == "__new__"
    {
        let Some(&class) = rest.first() else {
            return Err(Error::new(
                "TypeError",
                format!("_io.{kind}.__new__(): not enough arguments"),
            ));
        };
        if !ctx.direct_subclass(class, owner) {
            return Err(ctx.invalid_new_subtype(&format!("_io.{kind}"), class));
        }
        return Ok(ctx.instance(class, None));
    }
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
            (
                Some(
                    class @ ("BytesIO" | "BufferedReader" | "BufferedWriter" | "BufferedRWPair"
                    | "BufferedRandom"),
                ),
                "__init__",
            ) => {
                format!(
                    "descriptor '__init__' requires a '_io.{class}' object but received a '{actual}'"
                )
            }
            (Some("TextIOWrapper"), name @ ("__next__" | "__repr__")) => {
                let actual = receiver_class
                    .and_then(|class| ctx.heap.get(class).io_kind())
                    .map_or_else(|| actual.to_string(), |_| format!("_io.{actual}"));
                format!(
                    "descriptor '{name}' requires a '_io.TextIOWrapper' object but received a '{actual}'"
                )
            }
            (Some(class), _) => format!(
                "descriptor '{name}' for '_io.{class}' objects doesn't apply to a '{actual}' object"
            ),
            _ => format!("invalid receiver for {name}"),
        };
        return Err(Error::new("TypeError", message));
    };
    if owner_kind == Some("TextIOWrapper") && name == "__repr__" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "wrapper __repr__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        return text_wrapper::repr(ctx, receiver);
    }
    match owner_kind {
        Some("TextIOWrapper")
            if name == "__next__" && matches!(ctx.heap.get(receiver), Value::Instance { .. }) =>
        {
            return text_wrapper::next(ctx, receiver, receiver_class == owner, rest, kwargs);
        }
        Some("BufferedRandom") => return native_random::call(ctx, receiver, name, rest, kwargs),
        Some("BufferedReader") => {
            return native_reader::call(ctx, "BufferedReader", receiver, name, rest, kwargs);
        }
        Some("BufferedWriter") => {
            return native_writer::call(ctx, "BufferedWriter", receiver, name, rest, kwargs);
        }
        Some("BufferedRWPair") => return rwpair::call(ctx, owner, receiver, name, rest, kwargs),
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
                super::super::unraisable::report(
                    ctx,
                    error,
                    true,
                    Some(receiver),
                    "Exception ignored while finalizing file",
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
                    | "BufferedRandom"
                    | "BufferedRWPair"
            )
        });
    if layout == Some("BufferedRWPair") && matches!(name, "__enter__" | "__exit__") {
        return rwpair::call(ctx, owner, receiver, name, rest, kwargs);
    }
    if layout == Some("BufferedRandom") && matches!(name, "__enter__" | "__exit__") {
        return native_random::call(ctx, receiver, name, rest, kwargs);
    }
    if layout == Some("BufferedReader") && matches!(name, "__enter__" | "__exit__" | "writable") {
        return native_reader::call(ctx, "BufferedReader", receiver, name, rest, kwargs);
    }
    if layout == Some("BufferedWriter") && matches!(name, "__enter__" | "__exit__" | "readable") {
        return native_writer::call(ctx, "BufferedWriter", receiver, name, rest, kwargs);
    }
    if layout == Some("TextIOWrapper") {
        if name == "truncate" {
            return text_wrapper::truncate(ctx, receiver, rest, kwargs);
        }
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

fn string_io_getstate(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let file = string_io_file(ctx, receiver)?;
    let value = file.memory_value()?;
    let newline = file.memory_newline()?;
    let position = file.memory_position()?;
    let value = ctx.alloc(value.into());
    let newline = newline.map_or(NONE, |s| ctx.text(s));
    let position = ctx.int(position as i64);
    let dict = copy_stream_dict(ctx, receiver)?;
    Ok(ctx.alloc(Value::Tuple(vec![value, newline, position, dict])))
}

fn copy_stream_dict(ctx: &mut Context, receiver: Id) -> Result<Id> {
    if let Value::Instance {
        namespace: Some(dict),
        ..
    } = *ctx.heap.get(receiver)
    {
        let storage = ctx.dict_storage(dict).expect("instance dictionary");
        super::builtins::copy_dict(ctx, dict, storage)
    } else {
        Ok(NONE)
    }
}

fn string_io_setstate(ctx: &mut Context, receiver: Id, state: Id) -> Result<Id> {
    if string_io_file(ctx, receiver).is_ok_and(|file| file.closed()) {
        return Err(Error::new("ValueError", "I/O operation on closed file"));
    }
    let items = ctx.tuple_items(state)?;
    let Some(&[value, newline, position, dict, ..]) = items.as_deref() else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.__setstate__ argument should be 4-tuple, got {}",
                ctx.type_name(receiver),
                ctx.type_name(state),
            ),
        ));
    };
    string_io_init(ctx, receiver, &[value, newline], &[])?;
    let file = string_io_file(ctx, receiver)?;
    // Initialization sets the decoder history. The saved text has already been translated.
    match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(text) => file.replace_memory_value(text)?,
        Value::SurrogateStr(points) => file.replace_memory_points(points)?,
        _ => debug_assert_eq!(value, NONE),
    }
    let position = ctx
        .as_int(position)
        .map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "third item of state must be an integer, got {}",
                    ctx.type_name(position),
                ),
            )
        })?
        .to_i64()
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    if position < 0 {
        return Err(Error::new(
            "ValueError",
            "position value cannot be negative",
        ));
    }
    file.seek_memory(position, 0)?;
    restore_stream_dict(ctx, receiver, dict, "fourth")
}

fn restore_stream_dict(ctx: &mut Context, receiver: Id, dict: Id, item: &str) -> Result<Id> {
    if dict != NONE {
        if ctx.dict_storage(dict).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{item} item of state should be a dict, got a {}",
                    ctx.type_name(dict),
                ),
            ));
        }
        let Value::Instance { namespace, .. } = ctx.heap.get_mut(receiver) else {
            unreachable!("memory stream methods have an instance receiver");
        };
        if let Some(existing) = *namespace {
            let storage = ctx.dict_storage(existing).expect("instance dictionary");
            super::builtins::update_dict(ctx, storage, Some(dict), &[])?;
        } else {
            *namespace = Some(dict);
        }
    }
    Ok(NONE)
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
    if name == "__getstate__" {
        return string_io_getstate(ctx, receiver);
    }
    if name == "__setstate__" {
        return string_io_setstate(ctx, receiver, args[0]);
    }
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
