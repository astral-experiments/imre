//! Unbuffered filesystem streams for runtime-loaded Python code.

use std::cell::{Cell, RefCell};
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, IntoRawFd, RawFd};
use std::rc::Rc;

use super::super::{builtins, files, os::stat::warn_bool_fd, os_error};
use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::dynamic::value::IoOpenTypes;
use crate::objects::{BoxedPipe, os};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__repr__",
    "closed",
    "mode",
    "closefd",
    "_blksize",
    "close",
    "fileno",
    "read",
    "readall",
    "readinto",
    "write",
    "seek",
    "tell",
    "truncate",
    "readable",
    "writable",
    "seekable",
    "isatty",
    "_dealloc_warn",
];

pub(in crate::dynamic) struct RawFile {
    fd: Cell<Option<RawFd>>,
    pipe: RefCell<Option<BoxedPipe>>,
    closefd: Cell<bool>,
    python_finalized: Cell<bool>,
    mode: Cell<RawMode>,
    blksize: Cell<i64>,
    repr_active: Cell<bool>,
}

impl Drop for RawFile {
    fn drop(&mut self) {
        // A Python finalizer may have reused the number or left it to another
        // owner. Disarm the pipe's Rust File as well as the raw wrapper.
        if let Some(fd) = self.take_close_fd()
            && !self.python_finalized.get()
        {
            let _ = os::close(fd);
        }
    }
}

/// Use CPython's default when the filesystem reports an unusable block size.
fn file_block_size(size: impl TryInto<i64>) -> i64 {
    match size.try_into() {
        Ok(size) if size > 1 => size,
        _ => super::DEFAULT_BUFFER_SIZE,
    }
}

/// Truncate a live raw or buffered disk descriptor without changing its position.
pub(super) fn truncate_fd(fd: RawFd, size: i64) -> Result<()> {
    os::truncate(fd, size).map_err(os_error)
}

pub(super) fn closed() -> Error {
    Error::new("ValueError", "I/O operation on closed file")
}

impl RawFile {
    pub(super) fn live_fd(&self) -> Option<RawFd> {
        self.fd
            .get()
            .filter(|_| self.pipe.borrow().as_ref().is_none_or(|p| !p.closed()))
    }

    fn access<T>(&self, operation: impl FnOnce(RawFd) -> Result<T>) -> Result<T> {
        operation(self.live_fd().ok_or_else(closed)?)
    }

    pub(super) fn readable_fd(&self) -> Result<RawFd> {
        let fd = self.live_fd().ok_or_else(closed)?;
        self.check_direction(true)?;
        Ok(fd)
    }

    fn take_close_fd(&self) -> Option<RawFd> {
        let fd = self.fd.take()?;
        if !self.closefd.get() {
            return None;
        }
        self.blksize.set(super::DEFAULT_BUFFER_SIZE);
        match self.pipe.borrow().as_ref() {
            Some(pipe) => pipe.take_raw_fd(),
            None => Some(fd),
        }
    }

    pub(super) fn close(&self) -> Result<()> {
        if let Some(fd) = self.take_close_fd() {
            os::close(fd)?;
        }
        Ok(())
    }

    fn readable(&self) -> bool {
        self.mode.get().readable
    }

    fn writable(&self) -> bool {
        self.mode.get().writable
    }

    pub(super) fn check_direction(&self, read: bool) -> Result<()> {
        let allowed = if read {
            self.readable()
        } else {
            self.writable()
        };
        if allowed {
            Ok(())
        } else {
            Err(Error::new(
                "UnsupportedOperation",
                if read {
                    "File not open for reading"
                } else {
                    "File not open for writing"
                },
            ))
        }
    }
}

pub(in crate::dynamic) fn hand_file_finalization_to_python(ctx: &Context, receiver: Id) {
    if let Some(raw) = state(ctx, receiver) {
        raw.python_finalized.set(true);
    }
}

pub(super) fn state(ctx: &Context, receiver: Id) -> Option<Rc<RawFile>> {
    match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Raw(raw)),
            ..
        } => Some(raw.clone()),
        _ => None,
    }
}

/// Resolve the native FileIO.read without binding a fresh method for every byte.
pub(super) fn native_reader(ctx: &Context, receiver: Id) -> Option<Rc<RawFile>> {
    let raw = state(ctx, receiver)?;
    let class = ctx.heap.get(receiver).instance_class()?;
    if ctx.raw_instance_field(receiver, "read").is_some() {
        return None;
    }
    let method = ctx.class_attr(class, "read")?;
    let Value::IoMethod {
        owner,
        name: "read",
        ..
    } = ctx.heap.get(method)
    else {
        return None;
    };
    if ctx.heap.get(*owner).io_kind() != Some("FileIO")
        || (class != *owner
            && ctx.class_attr(class, "__getattribute__")
                != ctx.class_attr(*owner, "__getattribute__"))
    {
        return None;
    }
    Some(raw)
}

/// FileIO retains its access bits across explicit reinitialization.
#[derive(Clone, Copy, Default)]
struct RawMode {
    readable: bool,
    writable: bool,
    appending: bool,
    created: bool,
}

impl RawMode {
    fn parse(&mut self, raw: &str) -> Result<u32> {
        const INVALID: &str =
            "Must have exactly one of create/read/write/append mode and at most one plus";
        let mut selected = None;
        let mut plus = false;
        for ch in raw.chars() {
            match ch {
                'b' => continue,
                '+' if !plus => {
                    plus = true;
                    self.readable = true;
                    self.writable = true;
                }
                '+' => return Err(Error::new("ValueError", INVALID)),
                'r' | 'w' | 'a' | 'x' if selected.is_some() => {
                    return Err(Error::new("ValueError", INVALID));
                }
                'r' => {
                    self.readable = true;
                    selected = Some(0);
                }
                'w' => {
                    self.writable = true;
                    selected = Some(1);
                }
                'a' => {
                    self.writable = true;
                    self.appending = true;
                    selected = Some(2);
                }
                'x' => {
                    self.writable = true;
                    self.created = true;
                    selected = Some(3);
                }
                _ => return Err(Error::new("ValueError", format!("invalid mode: {raw}"))),
            }
        }
        selected.ok_or_else(|| Error::new("ValueError", INVALID))
    }

    fn spelling(self) -> &'static str {
        match (self.created, self.appending, self.readable, self.writable) {
            (true, _, true, _) => "xb+",
            (true, _, false, _) => "xb",
            (_, true, true, _) => "ab+",
            (_, true, false, _) => "ab",
            (_, _, true, true) => "rb+",
            (_, _, true, false) => "rb",
            (_, _, false, _) => "wb",
        }
    }
}

/// Parse a mode for a newly allocated raw stream.
pub(in crate::dynamic::stdlib) fn mode(raw: &str) -> Result<(u32, &'static str)> {
    let mut access = RawMode::default();
    let flags = access.parse(raw)?;
    Ok((flags, access.spelling()))
}

pub(in crate::dynamic::stdlib) fn validate_open_mode(mode: &str) -> Result<()> {
    let binary = mode.contains('b');
    if mode
        .chars()
        .any(|c| !"rwaxbt+".contains(c) || mode.matches(c).count() != 1)
    {
        return Err(Error::new("ValueError", format!("invalid mode: '{mode}'")));
    }
    if binary && mode.contains('t') {
        return Err(Error::new(
            "ValueError",
            "can't have text and binary mode at once",
        ));
    }
    if mode.bytes().filter(|c| b"rwax".contains(c)).count() > 1 {
        return Err(Error::new(
            "ValueError",
            "must have exactly one of create/read/write/append mode",
        ));
    }
    Ok(())
}

pub(in crate::dynamic::stdlib) fn check_binary_options(
    encoding: Option<Id>,
    errors: Option<Id>,
    newline: Option<Id>,
) -> Result<()> {
    for (name, value, article) in [
        ("encoding", encoding, "an"),
        ("errors", errors, "an"),
        ("newline", newline, "a"),
    ] {
        if value.is_some() {
            return Err(Error::new(
                "ValueError",
                format!("binary mode doesn't take {article} {name} argument"),
            ));
        }
    }
    Ok(())
}

pub(in crate::dynamic::stdlib) fn check_open_string(
    ctx: &mut Context,
    value: Id,
    name: &str,
    expected: &str,
) -> Result<()> {
    check_string_arg(ctx, value, "open", name, expected)
}

pub(super) fn check_string_arg(
    ctx: &mut Context,
    value: Id,
    function: &str,
    name: &str,
    expected: &str,
) -> Result<()> {
    let storage = ctx.string_storage(value).ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "{function}() argument '{name}' must be {expected}, not {}",
                if value == NONE {
                    "None"
                } else {
                    ctx.type_name(value)
                }
            ),
        )
    })?;
    if matches!(ctx.heap.get(storage), Value::SurrogateStr(_)) {
        ctx.encode_utf8_codepoints(value, false, "strict")?;
    }
    if ctx.str_value(storage)?.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    Ok(())
}

/// Convert open()'s optional text arguments before any path or descriptor is acquired.
pub(in crate::dynamic::stdlib) fn check_text_options(
    ctx: &mut Context,
    encoding: Option<Id>,
    errors: Option<Id>,
    newline: Option<Id>,
) -> Result<()> {
    for (name, id) in [
        ("encoding", encoding),
        ("errors", errors),
        ("newline", newline),
    ] {
        if let Some(id) = id.filter(|&id| id != NONE) {
            check_open_string(ctx, id, name, "str or None")?;
        }
    }
    Ok(())
}

enum DescriptorLookup {
    FromArgument,
    Resolved(Option<RawFd>),
}

/// FileIO falls back to the filesystem protocol when conversion to a C descriptor fails.
fn descriptor_from_argument(ctx: &mut Context, file: Id) -> Option<RawFd> {
    ctx.index_value(file)
        .ok()
        .flatten()
        .and_then(|n| n.to_i64())
        .and_then(|n| RawFd::try_from(n).ok())
}

/// FileIO encodes path spellings that the UTF-8 buffered-file shortcut cannot use.
fn needs_encoded_path(ctx: &Context, path: Id) -> Result<bool> {
    Ok(match ctx.heap.get(ctx.scalar_value(path)) {
        Value::Bytes(_) | Value::SurrogateStr(_) => true,
        Value::Str(text) if !text.is_ascii() => {
            crate::sys::filesystem_encoding()?.as_str() != "utf-8"
        }
        _ => false,
    })
}

/// Pass FileIO only its operation and optional update marker, in that order.
fn raw_open_mode(mode: &str) -> String {
    let mut raw: String = mode.chars().filter(|c| "rwax".contains(*c)).collect();
    if mode.contains('+') {
        raw.push('+');
    }
    raw
}

/// Builtins.open keeps using the initial _io types after either module is reimported.
fn open_types(ctx: &mut Context, fresh: Option<IoOpenTypes>) -> Result<IoOpenTypes> {
    if let Some(types) = fresh {
        return Ok(types);
    }
    if !ctx.builtins.contains_key("_io.FileIO") {
        ctx.module("_io")?;
    }
    let class = |name: &str| {
        ctx.builtins
            .get(name)
            .copied()
            .ok_or_else(|| Error::new("ImportError", format!("missing private type {name}")))
    };
    Ok(IoOpenTypes {
        file: class("_io.FileIO")?,
        reader: class("_io.BufferedReader")?,
        writer: class("_io.BufferedWriter")?,
        random: class("_io.BufferedRandom")?,
        wrapper: class("_io.TextIOWrapper")?,
    })
}

/// Resolve the public open() arguments before opening the underlying file.
pub(in crate::dynamic) fn open(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    fresh: Option<IoOpenTypes>,
) -> Result<Id> {
    let [
        file,
        mode,
        buffering,
        encoding,
        errors,
        newline,
        closefd,
        opener,
    ] = builtins::bind(
        "open",
        args,
        kwargs,
        [
            "file",
            "mode",
            "buffering",
            "encoding",
            "errors",
            "newline",
            "closefd",
            "opener",
        ],
    )?;
    let file = file.ok_or_else(|| Error::new("TypeError", "open requires a file"))?;
    if let Some(mode) = mode {
        check_open_string(ctx, mode, "mode", "str")?;
    }
    let buffering = buffering
        .map(|value| ctx.index_integer(value))
        .transpose()?
        .map_or(Some(-1), |n| n.to_i64().and_then(|n| i32::try_from(n).ok()))
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;

    let encoding = encoding.filter(|&v| v != NONE);
    let errors = errors.filter(|&v| v != NONE);
    let newline = newline.filter(|&v| v != NONE);
    check_text_options(ctx, encoding, errors, newline)?;
    let owns = closefd.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    // Numeric inputs reach FileIO before Python attempts their index or filesystem protocol.
    let numeric = matches!(
        ctx.heap.get(ctx.scalar_value(file)),
        Value::Int(_) | Value::Bool(_) | Value::Float(_) | Value::Complex(_)
    ) || ["__index__", "__int__", "__float__"]
        .iter()
        .any(|name| ctx.slot(file, name).is_some());
    let raw_arg = if numeric {
        file
    } else {
        super::super::posixpath::fspath(ctx, file)?
    };
    let mode_text = mode.map(|v| ctx.str_value(v)).transpose()?.unwrap_or("r");
    validate_open_mode(mode_text)?;
    let binary = mode_text.contains('b');
    if binary {
        check_binary_options(encoding, errors, newline)?;
    }
    let opener = opener.filter(|&v| v != NONE);
    if buffering == 0 {
        let raw_mode = raw_open_mode(mode_text);
        let class = open_types(ctx, fresh)?.file;
        return open_raw(ctx, class, raw_arg, raw_mode, binary, owns, opener);
    }
    let update = mode_text.contains('+');
    let sized_reader = !numeric && buffering > 1 && matches!(mode_text, "rb" | "br");
    let text_input = matches!(mode_text, "r" | "rt" | "tr");
    let text_output = !binary && !update && mode_text.contains(['w', 'a', 'x']);
    if binary && buffering == 1 {
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::RuntimeWarning,
            "line buffering (buffering=1) isn't supported in binary mode, the default buffer size will be used",
            1,
        )?;
    }
    // Sealed binary handles do not dispatch ResourceWarning from a Python finalizer.
    let warning_options = ctx.config["warnoptions"]
        .as_array()
        .is_some_and(|options| !options.is_empty());
    let fd = descriptor_from_argument(ctx, raw_arg);
    if fresh.is_some()
        || opener.is_some()
        || numeric
        || fd.is_some()
        || text_output
        || text_input
        || sized_reader
        || update
        || (binary && (buffering == 1 || warning_options))
        || (binary && !numeric && needs_encoded_path(ctx, raw_arg)?)
    {
        if fd.is_some_and(|fd| fd < 0) {
            return Err(Error::new("ValueError", "negative file descriptor"));
        }
        let mode_text = mode.map(|v| ctx.str_value(v)).transpose()?.unwrap_or("r");
        let reading = mode_text.contains('r');
        let raw_mode = raw_open_mode(mode_text);
        let types = open_types(ctx, fresh)?;
        let raw = ctx.instance(types.file, None);
        initialize_file(
            ctx,
            raw,
            raw_arg,
            &raw_mode,
            owns,
            DescriptorLookup::Resolved(fd),
            opener,
            None,
        )?;
        let handle = state(ctx, raw).expect("initialized FileIO");
        let capacity = if buffering > 1 {
            buffering as usize
        } else {
            crate::text_output::binary_capacity(handle.blksize.get().into())
        };
        let line_buffering =
            buffering == 1 || (!binary && buffering < 0 && handle.fd.get().is_some_and(os::isatty));
        let buffer = if fresh.is_some() || (reading && !update) || (binary && warning_options) {
            let class = if update {
                types.random
            } else if reading {
                types.reader
            } else {
                types.writer
            };
            let size = ctx.int(capacity as i64);
            ctx.call(class, vec![raw, size], Vec::new())
        } else {
            let buffer = if update {
                super::BufferedFd::update(raw, handle, capacity)
            } else {
                super::BufferedFd::new(raw, handle, reading, capacity)
            };
            buffer.and_then(|buffer| allocate_buffer(ctx, buffer))
        }
        .map_err(|error| close_on_error(ctx, raw, error))?;
        if binary {
            return Ok(buffer);
        }
        let stream = ctx
            .call(
                types.wrapper,
                vec![
                    buffer,
                    encoding.unwrap_or(NONE),
                    errors.unwrap_or(NONE),
                    newline.unwrap_or(NONE),
                    builtins::boolean(line_buffering),
                ],
                Vec::new(),
            )
            .map_err(|error| close_on_error(ctx, buffer, error))?;
        // open() gives TextIOWrapper a new exact str preserving the caller's spelling.
        let spelling = mode
            .map(|id| ctx.str_value(id))
            .transpose()?
            .unwrap_or("r")
            .to_owned();
        let mode = ctx.text(spelling);
        ctx.set_attr(stream, "mode", mode)
            .map_err(|error| close_on_error(ctx, stream, error))?;
        return Ok(stream);
    }
    let mode = mode.map(|v| ctx.str_value(v)).transpose()?.unwrap_or("r");
    let writer_capacity = match (buffering, mode) {
        (2.., "wb" | "ab" | "xb" | "bw" | "ba" | "bx") => Some(buffering as usize),
        (1.., _) => {
            return Err(Error::new(
                "ValueError",
                "explicit buffer sizes are unsupported",
            ));
        }
        _ => None,
    };
    if errors
        .map(|v| ctx.str_value(v))
        .transpose()?
        .is_some_and(|v| v != "strict")
        || newline.is_some()
    {
        return Err(Error::new(
            "NotImplementedError",
            "non-default text errors or newline are unsupported",
        ));
    }
    let encoding = encoding.map(|v| ctx.str_value(v)).transpose()?;
    if !matches!(
        crate::objects::Codec::for_stream(encoding)?,
        crate::objects::Codec::Utf8 { signature: false }
    ) {
        return Err(Error::new(
            "ValueError",
            "text files require UTF-8 encoding",
        ));
    }
    let (access, mode) = match mode {
        "rb" => (0, "rb"),
        "wb" | "bw" => (1, "wb"),
        "ab" | "ba" => (2, "ab"),
        "xb" | "bx" => (3, "xb"),
        _ => {
            return Err(Error::new(
                "ValueError",
                "unsupported mode for dynamic files",
            ));
        }
    };
    let pathname = super::super::os::c_path(ctx, raw_arg, "embedded null byte")?;
    let flags = crate::objects::file_flags(access, false);
    audit_open(ctx, raw_arg, &mode[..1], flags)?;
    if !owns {
        return Err(Error::new(
            "ValueError",
            "Cannot use closefd=False with file name",
        ));
    }
    let fd = super::super::os::open_path(ctx, raw_arg, &pathname, flags, 0o666, libc::AT_FDCWD)?;
    let path = ctx.str_value(raw_arg)?;
    let file = files::BinaryFile::from_open(path, raw_arg, access, mode, writer_capacity, fd)
        .and_then(files::BinaryFile::into_value);
    match file {
        Ok(file) => ctx.heap.alloc_descriptor_owner(file),
        Err(error) => Err(ctx.with_open_filename(error, raw_arg)?),
    }
}

fn open_raw(
    ctx: &mut Context,
    class: Id,
    filename: Id,
    raw_mode: String,
    binary: bool,
    owns: bool,
    opener: Option<Id>,
) -> Result<Id> {
    // CPython creates/truncates the raw file before rejecting unbuffered text.
    // The reported mode of w+ is rb+, but the constructor must still create/truncate.
    let mode = ctx.text(raw_mode);
    let file = ctx.call(
        class,
        vec![
            filename,
            mode,
            builtins::boolean(owns),
            opener.unwrap_or(NONE),
        ],
        Vec::new(),
    )?;
    if !binary {
        let error = Error::new("ValueError", "can't have unbuffered text I/O");
        return Err(close_on_error(ctx, file, error));
    }
    Ok(file)
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [path, mode_arg, closefd, opener] = builtins::bind(
        "FileIO",
        args,
        kwargs,
        ["file", "mode", "closefd", "opener"],
    )?;
    let path = path.ok_or_else(|| {
        Error::new(
            "TypeError",
            "FileIO() missing required argument 'file' (pos 1)",
        )
    })?;
    let selected_mode = if let Some(mode) = mode_arg {
        check_string_arg(ctx, mode, "FileIO", "mode", "str")?;
        ctx.str_value(mode)?.to_owned()
    } else {
        String::from("r")
    };
    let owns = closefd.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    initialize_file(
        ctx,
        receiver,
        path,
        &selected_mode,
        owns,
        DescriptorLookup::FromArgument,
        opener.filter(|&id| id != NONE),
        None,
    )
}

/// Close the last successfully constructed layer and preserve a secondary close failure.
fn close_on_error(ctx: &mut Context, layer: Id, error: Error) -> Error {
    let original = ctx.raised_instance(error.clone());
    ctx.heap.keep(original);
    let result = ctx
        .lookup_attr(layer, "close")
        .and_then(|close| ctx.call(close, Vec::new(), Vec::new()));
    if let Err(mut closing) = result {
        closing.context = original;
        closing
    } else {
        error
    }
}

fn allocate_buffer(ctx: &mut Context, buffer: super::BufferedFd) -> Result<Id> {
    let value = files::BinaryFile::Descriptor(buffer).into_value()?;
    ctx.heap.alloc_descriptor_owner(value)
}

impl Context {
    /// Expose a secure temporary file's read/write descriptor through a write-only buffer.
    pub(in crate::dynamic) fn temporary_writer(
        &mut self,
        file: std::fs::File,
        name: Id,
    ) -> Result<Id> {
        self.module("io")?;
        let fd = file.as_raw_fd();
        let raw = self.instance(self.builtins["_io.FileIO"], None);
        initialize_file(
            self,
            raw,
            name,
            "wb",
            true,
            DescriptorLookup::Resolved(Some(fd)),
            None,
            None,
        )?;
        // FileIO takes ownership of a passed descriptor only after successful initialization.
        let _ = file.into_raw_fd();
        let handle = state(self, raw).expect("initialized FileIO");
        let capacity = crate::text_output::binary_capacity(handle.blksize.get().into());
        let result = super::BufferedFd::new(raw, handle.clone(), false, capacity)
            .and_then(|buffer| allocate_buffer(self, buffer));
        if result.is_err() {
            handle.close()?;
        }
        result
    }
}

/// Expose the child stream and the native communication cursor through the same file.
pub(in crate::dynamic) fn popen_pipe(
    ctx: &mut Context,
    file: crate::ObjectOwner,
    reading: bool,
) -> Result<Id> {
    ctx.module("io")?;
    let pipe = BoxedPipe::from_owner(file);
    let fd = pipe.fileno()?;
    let number = ctx.int(i64::from(fd));
    let raw = ctx.instance(ctx.builtins["_io.FileIO"], None);
    let mode = if reading { "rb" } else { "wb" };
    initialize_file(
        ctx,
        raw,
        number,
        mode,
        true,
        DescriptorLookup::Resolved(Some(fd)),
        None,
        Some(pipe.clone()),
    )?;
    if pipe.is_raw() {
        return Ok(raw);
    }
    if reading {
        let receiver = ctx.instance(ctx.builtins["_io.BufferedReader"], None);
        let bytes = pipe.take_input_buffer()?;
        super::buffered_reader::install_buffer(ctx, receiver, raw, bytes, false)?;
        return Ok(receiver);
    }
    let handle = state(ctx, raw).expect("initialized FileIO");
    let buffer = super::BufferedFd::pipe(raw, handle, reading, pipe);
    ctx.heap
        .alloc_descriptor_owner(files::BinaryFile::Descriptor(buffer).into_value()?)
}

/// Validate an fd returned by a Python opener, which must be an actual integer.
fn call_opener(ctx: &mut Context, callable: Id, path: Id, flags: Id) -> Result<RawFd> {
    let result = ctx.call(callable, vec![path, flags], Vec::new())?;
    let integer = ctx
        .as_int(result)
        .map_err(|_| Error::new("TypeError", "expected integer from opener"))?;
    let fd = integer
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    if fd < 0 {
        return Err(Error::new("ValueError", format!("opener returned {fd}")));
    }
    Ok(fd)
}

fn audit_open(ctx: &mut Context, path: Id, mode: &str, flags: i32) -> Result<()> {
    if !ctx.audit_hooks.is_empty() {
        let spelling = ctx.text(mode);
        let word = ctx.int(i64::from(flags));
        super::super::audit::event(ctx, "open", &[path, spelling, word])?;
    }
    Ok(())
}

fn descriptor_block_size(ctx: &mut Context, path: Id, fd: RawFd) -> Result<i64> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat accepts any descriptor number and writes into this buffer.
    let blksize = if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == 0 {
        // SAFETY: fstat initialized stat on success.
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let error = os_error(std::io::Error::from_raw_os_error(libc::EISDIR));
            return Err(ctx.with_errno_filename(error, path)?);
        }
        file_block_size(stat.st_blksize)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EBADF) {
            return Err(os_error(err));
        }
        super::DEFAULT_BUFFER_SIZE
    };
    Ok(blksize)
}

#[allow(clippy::too_many_arguments)]
fn initialize_file(
    ctx: &mut Context,
    receiver: Id,
    path: Id,
    mode_text: &str,
    owns: bool,
    descriptor: DescriptorLookup,
    opener: Option<Id>,
    pipe: Option<BoxedPipe>,
) -> Result<Id> {
    // An opener, audit hook, or path protocol can reinitialize this same receiver.
    let raw = if let Some(raw) = state(ctx, receiver) {
        raw.close()?;
        *raw.pipe.borrow_mut() = pipe;
        raw
    } else {
        let raw = Rc::new(RawFile {
            fd: Cell::new(None),
            closefd: Cell::new(true),
            // A resurrected FileIO may receive its first descriptor after __del__.
            python_finalized: Cell::new(ctx.heap.finalizer_ran(receiver)),
            pipe: RefCell::new(pipe),
            mode: Cell::new(RawMode::default()),
            blksize: Cell::new(super::DEFAULT_BUFFER_SIZE),
            repr_active: Cell::new(false),
        });
        let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
            unreachable!("FileIO is an IOBase instance")
        };
        *memory_stream = Some(MemoryStream::Raw(raw.clone()));
        raw
    };
    warn_bool_fd(ctx, path)?;
    let descriptor = match descriptor {
        DescriptorLookup::FromArgument => descriptor_from_argument(ctx, path),
        DescriptorLookup::Resolved(fd) => fd,
    };
    let pathname = match descriptor {
        Some(fd) if fd < 0 => return Err(Error::new("ValueError", "negative file descriptor")),
        Some(_) => None,
        None => {
            let spelling = super::super::posixpath::fspath(ctx, path)?;
            Some(super::super::os::c_path(
                ctx,
                spelling,
                "embedded null byte",
            )?)
        }
    };
    // Only a pathname acquisition transfers ownership during initialization. A numeric
    // argument remains the caller's if a later check or the name setter fails.
    let mut acquired = false;
    let initialized = (|| {
        let mut access = raw.mode.get();
        let parsed = access.parse(mode_text);
        raw.mode.set(access);
        let flags = crate::objects::file_flags(parsed?, access.readable && access.writable);
        audit_open(ctx, path, mode_text, flags)?;
        let blksize = if let Some(filename) = pathname {
            raw.closefd.set(true);
            if !owns {
                return Err(Error::new(
                    "ValueError",
                    "Cannot use closefd=False with file name",
                ));
            }
            let fd = if let Some(callable) = opener {
                let word = ctx.int(i64::from(flags));
                call_opener(ctx, callable, path, word)?
            } else {
                super::super::os::open_path(ctx, path, &filename, flags, 0o666, libc::AT_FDCWD)?
                    .into_raw_fd()
            };
            raw.fd.set(Some(fd));
            acquired = true;
            if opener.is_some() {
                os::INHERITABLE.update(fd, false)?;
            }
            descriptor_block_size(ctx, path, fd)?
        } else {
            let fd = descriptor.expect("numeric FileIO argument");
            raw.closefd.set(owns);
            raw.fd.set(Some(fd));
            descriptor_block_size(ctx, path, fd)?
        };
        raw.blksize.set(blksize);
        ctx.set_attr(receiver, "name", path)?;
        if raw.mode.get().appending {
            // Seek without changing kernel flags, including if the opener returned a pipe.
            let active_fd = raw.live_fd().unwrap_or(-1);
            // SAFETY: lseek accepts any descriptor number and retains no pointers.
            if unsafe { libc::lseek(active_fd, 0, libc::SEEK_END) } < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESPIPE) {
                    return Err(os_error(err));
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = initialized {
        raw.blksize.set(super::DEFAULT_BUFFER_SIZE);
        if let Some(fd) = raw.fd.take()
            && acquired
            && let Err(close_error) = os::close(fd)
        {
            let mut close_error: Error = close_error.into();
            close_error.context = ctx.raised_instance(error);
            return Err(close_error);
        }
        return Err(error);
    }
    Ok(NONE)
}

/// A single FileIO write, including a successful short write to a pipe.
pub(super) fn write_once(fd: RawFd, bytes: &[u8]) -> std::io::Result<usize> {
    // SAFETY: the kernel reads at most the supplied slice and retains no pointer.
    let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if count < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(count as usize)
    }
}

/// Dispatch before retrying the original syscall. The destination can be Python storage,
/// so each attempt must release its borrow before a signal handler runs.
fn retry_read(
    ctx: &mut Context,
    mut attempt: impl FnMut(&mut Context) -> Result<Option<usize>>,
) -> Result<Option<usize>> {
    loop {
        match attempt(ctx) {
            Ok(None) => ctx.dispatch_signals()?,
            Err(error)
                if ctx.has_errno(&error, libc::EAGAIN)
                    || ctx.has_errno(&error, libc::EWOULDBLOCK) =>
            {
                return Ok(None);
            }
            result => return result,
        }
    }
}

/// Read into exported storage while a signal handler is free to inspect or rebind the raw.
pub(super) fn read_span(
    ctx: &mut Context,
    raw: &RawFile,
    storage: Id,
    range: std::ops::Range<usize>,
) -> Result<Option<usize>> {
    let fd = raw.readable_fd()?;
    retry_read(ctx, |ctx| {
        let bytes = Context::mutable_storage_bytes(ctx.heap.get_mut(storage))?;
        Ok(os::try_readinto(fd, &mut bytes[range.clone()])?)
    })
}

/// Perform one unbuffered read into the caller's storage, retrying the same descriptor on EINTR.
pub(super) fn read_once(ctx: &mut Context, fd: RawFd, buf: &mut [u8]) -> Result<Option<usize>> {
    retry_read(ctx, |_| Ok(os::try_readinto(fd, buf)?))
}

pub(super) fn read_all(ctx: &mut Context, raw: &RawFile, mut bytes: Vec<u8>) -> Result<Id> {
    let mut chunk = [0; 8192];
    loop {
        match read_once(ctx, raw.live_fd().unwrap_or(-1), &mut chunk)? {
            None if bytes.is_empty() => return Ok(NONE),
            None | Some(0) => break,
            Some(n) => {
                bytes
                    .try_reserve(n)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                bytes.extend_from_slice(&chunk[..n]);
            }
        }
    }
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        return initialize(ctx, receiver, args, kwargs);
    }
    let arity = match name {
        "write" | "readinto" | "_dealloc_warn" => 1..=1,
        "read" | "truncate" => 0..=1,
        "seek" => 1..=2,
        "__exit__" => 3..=3,
        _ => 0..=0,
    };
    if !kwargs.is_empty() || !arity.contains(&args.len()) {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to FileIO.{name}"),
        ));
    }
    if name == "write"
        && let Some(method) = ctx.python_buffer_method(args[0], true)?
    {
        // FileIO requests PyBUF_SIMPLE. Acquisition can close or replace its descriptor.
        return ctx.with_contiguous_python_buffer(args[0], method, 0, |ctx, view| {
            call_method(ctx, receiver, name, &[view])
        });
    }
    call_method(ctx, receiver, name, args)
}

fn call_method(ctx: &mut Context, receiver: Id, name: &str, args: &[Id]) -> Result<Id> {
    let raw = state(ctx, receiver);
    let is_closed = raw.as_ref().is_none_or(|raw| raw.live_fd().is_none());
    match name {
        "_dealloc_warn" => {
            let source = args[0];
            if !is_closed && raw.as_ref().is_some_and(|raw| raw.closefd.get()) {
                let result = ctx.checked_format(source, true).and_then(|repr| {
                    ctx.emit_builtin_warning_with_source(
                        crate::objects::exceptions::Kind::ResourceWarning,
                        &format!("unclosed file {repr}"),
                        1,
                        source,
                    )
                });
                if let Err(error) = result {
                    let warning = ctx.builtin("Warning");
                    if ctx.raised_error_matches(&error, warning) {
                        super::super::super::unraisable::report(
                            ctx,
                            error,
                            true,
                            Some(receiver),
                            "Exception ignored while finalizing file",
                        );
                    }
                }
            }
            return Ok(NONE);
        }
        "closed" => return Ok(builtins::boolean(is_closed)),
        "closefd" => {
            return Ok(builtins::boolean(
                raw.as_ref().is_none_or(|r| r.closefd.get()),
            ));
        }
        "mode" => return Ok(ctx.text(raw.as_ref().map_or("wb", |raw| raw.mode.get().spelling()))),
        "_blksize" => {
            return Ok(ctx.int(
                raw.as_ref()
                    .map_or(super::DEFAULT_BUFFER_SIZE, |r| r.blksize.get()),
            ));
        }
        "__repr__" => {
            let class = if ctx
                .heap
                .get(receiver)
                .instance_class()
                .and_then(|class| ctx.heap.get(class).io_kind())
                == Some("FileIO")
            {
                "_io.FileIO"
            } else {
                ctx.type_name(receiver)
            }
            .to_owned();
            if is_closed {
                return Ok(ctx.text(format!("<{class} [closed]>")));
            }
            let raw = raw.unwrap();
            if raw.repr_active.replace(true) {
                return Err(Error::new(
                    "RuntimeError",
                    format!("reentrant call inside {class}.__repr__"),
                ));
            }
            let shown = (|| {
                Ok(match ctx.lookup_attr(receiver, "name") {
                    Ok(path) => {
                        let repr = ctx.builtin("repr");
                        let shown = ctx.call(repr, vec![path], Vec::new())?;
                        format!("name={}", ctx.str_value(shown)?)
                    }
                    Err(err) if err.kind == "AttributeError" => {
                        let fd = raw.live_fd().ok_or_else(closed)?;
                        format!("fd={fd}")
                    }
                    Err(err) => return Err(err),
                })
            })();
            raw.repr_active.set(false);
            let shown: String = shown?;
            return Ok(ctx.text(format!(
                "<{class} {shown} mode='{}' closefd={}>",
                raw.mode.get().spelling(),
                if raw.closefd.get() { "True" } else { "False" }
            )));
        }
        "close" if is_closed => return Ok(NONE),
        "close" => {
            return super::buffered_io::flush_and_close(ctx, receiver, |ctx| {
                if let Some(current) = state(ctx, receiver) {
                    current.close()?;
                }
                Ok(NONE)
            });
        }
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            return ctx.call(close, Vec::new(), Vec::new());
        }
        "flush" | "__enter__" if is_closed => {
            return Err(Error::new("ValueError", "I/O operation on closed file."));
        }
        "flush" => return Ok(NONE),
        "__enter__" => return Ok(receiver),
        _ => {}
    }
    // Argument conversions may call back into this file and close or reinitialize it.
    let size = if name == "read" {
        files::read_length(ctx, args.first().copied())?
    } else {
        -1
    };
    if name == "write" {
        let input = args[0];
        ctx.check_buffer_exporter(input)?;
        ctx.native_buffer_bytes(input)?;
        let raw = raw.ok_or_else(closed)?;
        let fd = raw.live_fd().ok_or_else(closed)?;
        raw.check_direction(false)?;
        return ctx.with_buffer_export(input, |ctx| {
            loop {
                match write_once(fd, ctx.native_buffer_bytes(input)?) {
                    Ok(n) => return Ok(ctx.int(n as i64)),
                    Err(error) if error.kind() == ErrorKind::Interrupted => {
                        ctx.dispatch_signals()?
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(NONE),
                    Err(error) => return Err(os_error(error)),
                }
            }
        });
    }
    if name == "readinto" {
        return ctx.with_writable_byte_span(
            args[0],
            "readinto() argument 1",
            |ctx, storage, range| {
                let raw = state(ctx, receiver).ok_or_else(closed)?;
                let count = read_span(ctx, &raw, storage, range)?;
                Ok(count.map_or(NONE, |n| ctx.int(n as i64)))
            },
        );
    }
    let whence = if name == "seek" && args.len() == 2 {
        ctx.index_integer(args[1])?
            .to_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })?
    } else {
        0
    };
    let raw = state(ctx, receiver).ok_or_else(closed)?;
    if matches!(name, "seek" | "truncate") {
        raw.access(|_| {
            if name == "truncate" {
                raw.check_direction(false)
            } else {
                Ok(())
            }
        })?;
        let offset = args
            .first()
            .filter(|&&id| name == "seek" || id != NONE)
            .map(|&id| ctx.index_integer(id))
            .transpose()?;
        let offset = offset
            .map(|n| {
                n.to_i64().ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C long")
                })
            })
            .transpose()?;
        let raw = state(ctx, receiver).ok_or_else(closed)?;
        return raw.access(|fd| {
            if name == "seek" {
                return Ok(ctx.int(os::seek(
                    fd,
                    offset.ok_or_else(|| Error::new("TypeError", "an integer is required"))?,
                    whence,
                )?));
            }
            let size = match offset {
                Some(n) => n,
                None => os::seek(fd, 0, libc::SEEK_CUR)?,
            };
            truncate_fd(fd, size)?;
            raw.blksize.set(super::DEFAULT_BUFFER_SIZE);
            Ok(args
                .first()
                .copied()
                .filter(|&id| id != NONE)
                .unwrap_or_else(|| ctx.int(size)))
        });
    }
    if matches!(name, "read" | "readall") {
        let fd = raw.readable_fd()?;
        if size < 0 {
            return read_all(ctx, &raw, Vec::new());
        }
        let mut bytes = builtins::zeroed_bytes(size)?;
        let Some(count) = read_once(ctx, fd, &mut bytes)? else {
            return Ok(NONE);
        };
        bytes.truncate(count);
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    raw.access(|fd| match name {
        "readable" => Ok(builtins::boolean(raw.readable())),
        "writable" => Ok(builtins::boolean(raw.writable())),
        "fileno" => Ok(ctx.int(i64::from(fd))),
        "isatty" => Ok(builtins::boolean(os::isatty(fd))),
        "seekable" => Ok(builtins::boolean(os::seek(fd, 0, libc::SEEK_CUR).is_ok())),
        "tell" => Ok(ctx.int(os::seek(fd, 0, libc::SEEK_CUR)?)),
        _ => Err(Error::new(
            "AttributeError",
            format!("unsupported FileIO method {name}"),
        )),
    })
}
