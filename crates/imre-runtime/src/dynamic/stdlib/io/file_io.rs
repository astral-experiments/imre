//! Unbuffered filesystem streams for runtime-loaded Python code.

use std::cell::{Cell, RefCell};
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, IntoRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;

use super::super::{builtins, files, os::stat::warn_bool_fd, os_error};
use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::objects::{BoxedPipe, os};

pub(super) const METHODS: &[&str] = &[
    "__init__", "__repr__", "closed", "mode", "closefd", "_blksize", "close", "fileno", "read",
    "readall", "readline", "readinto", "write", "seek", "tell", "truncate", "readable", "writable",
    "seekable", "isatty", "_dealloc_warn",
];

pub(in crate::dynamic) struct RawFile {
    fd: Cell<Option<RawFd>>,
    pipe: RefCell<Option<BoxedPipe>>,
    closefd: Cell<bool>,
    mode: Cell<RawMode>,
    blksize: Cell<i64>,
    repr_active: Cell<bool>,
}

impl Drop for RawFile {
    fn drop(&mut self) {
        let _ = self.close();
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

fn closed() -> Error {
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

    pub(super) fn close(&self) -> Result<()> {
        if let Some(fd) = self.fd.take()
            && self.closefd.get()
        {
            self.blksize.set(super::DEFAULT_BUFFER_SIZE);
            if let Some(pipe) = self.pipe.borrow().as_ref() {
                pipe.close_raw()?;
            } else {
                os::close(fd)?;
            }
        }
        Ok(())
    }

    fn readable(&self) -> bool {
        self.mode.get().readable
    }

    fn writable(&self) -> bool {
        self.mode.get().writable
    }

    fn check_direction(&self, read: bool) -> Result<()> {
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

pub(super) fn state(ctx: &Context, receiver: Id) -> Option<Rc<RawFile>> {
    match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Raw(raw)),
            ..
        } => Some(raw.clone()),
        _ => None,
    }
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

/// Resolve the public open() arguments before opening the underlying file.
pub(in crate::dynamic) fn open(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
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
        return open_raw(ctx, raw_arg, raw_mode, binary, owns, opener);
    }
    let update = mode_text.contains('+');
    let text_output = !binary && !update && mode_text.contains(['w', 'a', 'x']);
    let text_input = buffering < 0 && matches!(mode_text, "r" | "rt" | "tr");
    if binary && buffering == 1 {
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::RuntimeWarning,
            "line buffering (buffering=1) isn't supported in binary mode, the default buffer size will be used",
            1,
        )?;
    }
    let fd = descriptor_from_argument(ctx, raw_arg);
    if opener.is_some()
        || fd.is_some()
        || text_output
        || text_input
        || update
        || (binary && buffering == 1)
        || (binary && !numeric && needs_encoded_path(ctx, raw_arg)?)
    {
        if fd.is_some_and(|fd| fd < 0) {
            return Err(Error::new("ValueError", "negative file descriptor"));
        }
        let mode_text = mode.map(|v| ctx.str_value(v)).transpose()?.unwrap_or("r");
        let reading = mode_text.contains('r');
        let raw_mode = raw_open_mode(mode_text);
        ctx.module("io")?;
        let raw = ctx.instance(ctx.builtins["_io.FileIO"], None);
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
        let buffer = if update {
            super::BufferedFd::update(raw, handle, capacity)
        } else {
            super::BufferedFd::new(raw, handle, reading, capacity)
        }
        .and_then(|buffer| allocate_buffer(ctx, buffer))
        .map_err(|error| close_on_error(ctx, raw, error))?;
        if binary {
            return Ok(buffer);
        }
        let wrapper = ctx.builtins["_io.TextIOWrapper"];
        let stream = ctx
            .call(
                wrapper,
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
    let filename = if numeric {
        super::super::posixpath::fspath(ctx, raw_arg)?
    } else {
        raw_arg
    };
    if !owns {
        return Err(Error::new(
            "ValueError",
            "Cannot use closefd=False with file name",
        ));
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
    let path = ctx.str_value(filename)?;
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
    let file = files::BinaryFile::open(path, filename, mode, writer_capacity)
        .and_then(files::BinaryFile::into_value);
    match file {
        Ok(file) => ctx.heap.alloc_file_writer(file),
        Err(error) => Err(ctx.with_open_filename(error, raw_arg)?),
    }
}

fn open_raw(
    ctx: &mut Context,
    filename: Id,
    raw_mode: String,
    binary: bool,
    owns: bool,
    opener: Option<Id>,
) -> Result<Id> {
    // CPython creates/truncates the raw file before rejecting unbuffered text.
    ctx.module("io")?;
    let class = ctx.builtins["_io.FileIO"];
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
    ctx.heap.alloc_file_writer(value)
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
    let handle = state(ctx, raw).expect("initialized FileIO");
    let buffer = super::BufferedFd::pipe(raw, handle, reading, pipe);
    ctx.heap
        .alloc_file_writer(files::BinaryFile::Descriptor(buffer).into_value()?)
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

/// Translate the parsed FileIO mode into the flags passed to an opener.
fn opener_flags(mode: u32, update: bool) -> i32 {
    let access = if update {
        libc::O_RDWR
    } else if mode == 0 {
        libc::O_RDONLY
    } else {
        libc::O_WRONLY
    };
    let create = match mode {
        0 => 0,
        1 => libc::O_CREAT | libc::O_TRUNC,
        2 => libc::O_CREAT | libc::O_APPEND,
        3 => libc::O_CREAT | libc::O_EXCL,
        _ => unreachable!("parsed FileIO mode"),
    };
    access | create | libc::O_CLOEXEC
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
        let flags = parsed?;
        let update = access.readable && access.writable;
        let opener = if let Some(callable) = opener {
            let spelling = ctx.text(mode_text);
            let word = ctx.int(i64::from(opener_flags(flags, update)));
            super::super::audit::event(ctx, "open", &[path, spelling, word])?;
            Some((callable, word))
        } else {
            None
        };
        let blksize = if let Some(filename) = pathname {
            raw.closefd.set(true);
            if !owns {
                return Err(Error::new(
                    "ValueError",
                    "Cannot use closefd=False with file name",
                ));
            }
            if let Some((callable, word)) = opener {
                let fd = call_opener(ctx, callable, path, word)?;
                raw.fd.set(Some(fd));
                acquired = true;
                os::INHERITABLE.update(fd, false)?;
                descriptor_block_size(ctx, path, fd)?
            } else {
                let (file, metadata) =
                    match crate::objects::open_raw_file(filename.as_bytes(), flags, update) {
                        Ok(opened) => opened,
                        Err(error) => return Err(ctx.with_errno_filename(error.into(), path)?),
                    };
                raw.fd.set(Some(file.into_raw_fd()));
                acquired = true;
                file_block_size(metadata.blksize())
            }
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

fn transfer(mut syscall: impl FnMut() -> isize) -> Result<Option<usize>> {
    loop {
        let count = syscall();
        if count >= 0 {
            return Ok(Some(count as usize));
        }
        let err = std::io::Error::last_os_error();
        match err.kind() {
            ErrorKind::Interrupted => continue,
            ErrorKind::WouldBlock => return Ok(None),
            _ => return Err(os_error(err)),
        }
    }
}

fn read_into(fd: RawFd, buf: &mut [u8]) -> Result<Option<usize>> {
    // SAFETY: read receives a writable slice and does not retain its pointer.
    transfer(|| unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) })
}

/// A signal handler can raise, close or reinitialize this FileIO before the retry.
fn read_interruptible(ctx: &mut Context, raw: &RawFile, buf: &mut [u8]) -> Result<Option<usize>> {
    loop {
        let fd = raw.live_fd().unwrap_or(-1);
        // SAFETY: the destination is an owned buffer, with no borrowed Python storage.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(Some(n as usize));
        }
        let err = std::io::Error::last_os_error();
        match err.kind() {
            ErrorKind::Interrupted => ctx.dispatch_signals()?,
            ErrorKind::WouldBlock => return Ok(None),
            _ => return Err(os_error(err)),
        }
    }
}

pub(super) fn read(
    ctx: &mut Context,
    raw: &RawFile,
    size: i64,
    line: bool,
    mut bytes: Vec<u8>,
) -> Result<Id> {
    if size >= 0 && !line {
        let mut bytes = builtins::zeroed_bytes(size)?;
        let Some(n) = read_interruptible(ctx, raw, &mut bytes)? else {
            return Ok(NONE);
        };
        bytes.truncate(n);
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let mut chunk = [0; 8192];
    while size < 0 || bytes.len() < size as usize {
        let cap = if line { 1 } else { chunk.len() };
        match read_interruptible(ctx, raw, &mut chunk[..cap])? {
            None if line => {
                return Err(Error::new(
                    "OSError",
                    "read() should have returned a bytes object, not 'NoneType'",
                ));
            }
            None if bytes.is_empty() => return Ok(NONE),
            None | Some(0) => break,
            Some(n) => {
                bytes
                    .try_reserve(n)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                bytes.extend_from_slice(&chunk[..n]);
                if line && chunk[0] == b'\n' {
                    break;
                }
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
        "read" | "readline" | "truncate" => 0..=1,
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
                            ctx, error, true, Some(receiver), "Exception ignored while finalizing file",
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
            let result = ctx
                .lookup_attr(receiver, "flush")
                .and_then(|f| ctx.call(f, Vec::new(), Vec::new()));
            if let Some(current) = state(ctx, receiver) {
                current.close()?;
            }
            return result.map(|_| NONE);
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
    let size = if matches!(name, "read" | "readline") {
        files::read_length(ctx, args.first().copied())?
    } else {
        -1
    };
    if name == "readline" && size == 0 {
        return Ok(ctx.alloc(Value::Bytes(Vec::new())));
    }
    if name == "write" {
        ctx.bytes_like(args[0])?;
    }
    if name == "readinto" && ctx.writable_byte_span(args[0])?.is_none() {
        return Err(Error::new(
            "TypeError",
            format!(
                "readinto() argument 1 must be read-write bytes-like object, not {}",
                ctx.type_name(args[0])
            ),
        ));
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
    raw.access(|fd| match name {
        "readable" => Ok(builtins::boolean(raw.readable())),
        "writable" => Ok(builtins::boolean(raw.writable())),
        "fileno" => Ok(ctx.int(i64::from(fd))),
        "isatty" => Ok(builtins::boolean(os::isatty(fd))),
        "seekable" => Ok(builtins::boolean(os::seek(fd, 0, libc::SEEK_CUR).is_ok())),
        "tell" => Ok(ctx.int(os::seek(fd, 0, libc::SEEK_CUR)?)),
        "read" | "readall" | "readline" => {
            raw.check_direction(true)?;
            read(ctx, &raw, size, name == "readline", Vec::new())
        }
        "readinto" => {
            raw.check_direction(true)?;
            let buf = files::readinto_bytes(ctx, args[0])?;
            Ok(read_into(fd, buf)?.map_or(NONE, |n| ctx.int(n as i64)))
        }
        "write" => {
            raw.check_direction(false)?;
            let bytes = ctx.bytes_like(args[0])?;
            // SAFETY: write reads only the live bytes slice for this call.
            Ok(
                transfer(|| unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) })?
                    .map_or(NONE, |n| ctx.int(n as i64)),
            )
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unsupported FileIO method {name}"),
        )),
    })
}
