//! Unbuffered filesystem streams for runtime-loaded Python code.

use std::cell::Cell;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, IntoRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;

use super::super::{builtins, files, os_error};
use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::objects::{BoxedPipe, os};

pub(super) const METHODS: &[&str] = &[
    "__init__", "__repr__", "closed", "mode", "closefd", "_blksize", "close", "fileno", "read",
    "readall", "readline", "readinto", "write", "seek", "tell", "truncate", "readable", "writable",
    "seekable", "isatty",
];

pub(in crate::dynamic) struct RawFile {
    fd: Cell<Option<RawFd>>,
    pipe: Option<BoxedPipe>,
    closefd: bool,
    mode: &'static str,
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

fn closed() -> Error {
    Error::new("ValueError", "I/O operation on closed file")
}

impl RawFile {
    pub(super) fn live_fd(&self) -> Option<RawFd> {
        self.fd
            .get()
            .filter(|_| self.pipe.as_ref().is_none_or(|p| !p.closed()))
    }

    fn access<T>(&self, operation: impl FnOnce(RawFd) -> Result<T>) -> Result<T> {
        operation(self.live_fd().ok_or_else(closed)?)
    }

    pub(super) fn close(&self) -> Result<()> {
        if let Some(fd) = self.fd.take()
            && self.closefd
        {
            self.blksize.set(super::DEFAULT_BUFFER_SIZE);
            if let Some(pipe) = &self.pipe {
                pipe.close_raw()?;
            } else {
                os::close(fd)?;
            }
        }
        Ok(())
    }

    fn readable(&self) -> bool {
        self.mode == "rb" || self.mode.ends_with('+')
    }

    fn writable(&self) -> bool {
        self.mode != "rb"
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

fn mode(raw: &str) -> Result<(u32, &'static str)> {
    const INVALID: &str =
        "Must have exactly one of create/read/write/append mode and at most one plus";
    let mut selected = None;
    let mut update = false;
    for ch in raw.chars() {
        let access = match ch {
            'r' => (0, "rb", "rb+"),
            'w' => (1, "wb", "rb+"),
            'a' => (2, "ab", "ab+"),
            'x' => (3, "xb", "xb+"),
            'b' => continue,
            '+' if !update => {
                update = true;
                continue;
            }
            '+' => return Err(Error::new("ValueError", INVALID)),
            _ => return Err(Error::new("ValueError", format!("invalid mode: {raw}"))),
        };
        if selected.replace(access).is_some() {
            return Err(Error::new("ValueError", INVALID));
        }
    }
    let (flags, single, both) = selected.ok_or_else(|| Error::new("ValueError", INVALID))?;
    Ok((flags, if update { both } else { single }))
}

fn validate_open_mode(mode: &str) -> Result<()> {
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

fn check_open_string(ctx: &mut Context, value: Id, name: &str, expected: &str) -> Result<()> {
    let storage = ctx.string_storage(value).ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "open() argument '{name}' must be {expected}, not {}",
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

/// Resolve the public open() arguments before opening the underlying file.
pub(in crate::dynamic) fn open(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [file, mode, buffering, encoding, errors, newline, closefd] = builtins::bind(
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
    for (name, id) in [
        ("encoding", encoding),
        ("errors", errors),
        ("newline", newline),
    ] {
        if let Some(id) = id {
            check_open_string(ctx, id, name, "str or None")?;
        }
    }
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
    }
    if buffering == 0 {
        let raw_mode = mode_text.replace('t', "");
        return open_raw(ctx, raw_arg, raw_mode, binary, owns);
    }
    let text_output = !binary && !mode_text.contains('+') && mode_text.contains(['w', 'a', 'x']);
    let text_input_with_newline =
        buffering < 0 && newline.is_some() && matches!(mode_text, "r" | "rt" | "tr");
    let fd = descriptor_from_argument(ctx, raw_arg);
    if fd.is_some() || text_output || text_input_with_newline {
        if fd.is_some_and(|fd| fd < 0) {
            return Err(Error::new("ValueError", "negative file descriptor"));
        }
        let mode_text = mode.map(|v| ctx.str_value(v)).transpose()?.unwrap_or("r");
        if mode_text.contains('+') {
            return Err(Error::new(
                "NotImplementedError",
                "buffered update descriptors are unsupported",
            ));
        }
        let reading = mode_text.contains('r');
        let raw_mode = mode_text.replace('t', "");
        if buffering > 0 {
            return Err(Error::new(
                "ValueError",
                "explicit buffer sizes are unsupported",
            ));
        }
        ctx.module("io")?;
        let raw = ctx.instance(ctx.builtins["_io.FileIO"], None);
        initialize_file(
            ctx,
            raw,
            raw_arg,
            self::mode(&raw_mode),
            owns,
            DescriptorLookup::Resolved(fd),
            None,
            None,
        )?;
        let handle = state(ctx, raw).expect("initialized FileIO");
        let buffer = buffered_descriptor(ctx, raw, handle, reading)?;
        if binary {
            return Ok(buffer);
        }
        let wrapper = ctx.builtins["_io.TextIOWrapper"];
        let result = ctx
            .call(
                wrapper,
                vec![
                    buffer,
                    encoding.unwrap_or(NONE),
                    errors.unwrap_or(NONE),
                    newline.unwrap_or(NONE),
                ],
                Vec::new(),
            )
            .and_then(|stream| {
                let mode = mode
                    .map(|id| ctx.scalar_value(id))
                    .unwrap_or_else(|| ctx.text("r"));
                ctx.set_attr(stream, "mode", mode)?;
                Ok(stream)
            });
        if result.is_err() {
            let _ = state(ctx, raw).expect("initialized FileIO").close();
        }
        return result;
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
    let file = match mode {
        "r" | "rt" | "tr" => crate::objects::TextFile::open(path, 0)
            .map(Value::TextFile)
            .map_err(Error::from),
        _ => files::BinaryFile::open(path, filename, mode, writer_capacity)
            .and_then(files::BinaryFile::into_value),
    };
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
) -> Result<Id> {
    // CPython creates/truncates the raw file before rejecting unbuffered text.
    ctx.module("io")?;
    let class = ctx.builtins["_io.FileIO"];
    // The reported mode of w+ is rb+, but the constructor must still create/truncate.
    let mode = ctx.text(raw_mode);
    let file = ctx.call(
        class,
        vec![filename, mode, builtins::boolean(owns)],
        Vec::new(),
    )?;
    if !binary {
        let raw = state(ctx, file).unwrap();
        raw.close()?;
        return Err(Error::new("ValueError", "can't have unbuffered text I/O"));
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
    let selected_mode = mode(
        mode_arg
            .map(|v| ctx.str_value(v))
            .transpose()?
            .unwrap_or("r"),
    );
    let owns = closefd.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    initialize_file(
        ctx,
        receiver,
        path,
        selected_mode,
        owns,
        DescriptorLookup::FromArgument,
        opener,
        None,
    )
}

fn buffered_descriptor(
    ctx: &mut Context,
    raw: Id,
    handle: Rc<RawFile>,
    reading: bool,
) -> Result<Id> {
    let capacity = crate::text_output::binary_capacity(handle.blksize.get().into());
    let buffer =
        files::BinaryFile::Descriptor(super::BufferedFd::new(raw, handle, reading, capacity))
            .into_value()?;
    ctx.heap.alloc_file_writer(buffer)
}

impl Context {
    /// Wrap mkstemp's read/write descriptor in a write-only buffered Python stream.
    pub(in crate::dynamic) fn named_temporary_writer(
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
            mode("wb"),
            true,
            DescriptorLookup::Resolved(Some(fd)),
            None,
            None,
        )?;
        // FileIO takes ownership of a passed descriptor only after successful initialization.
        let _ = file.into_raw_fd();
        let handle = state(self, raw).expect("initialized FileIO");
        let result = buffered_descriptor(self, raw, handle.clone(), false);
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
        self::mode(mode),
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

#[allow(clippy::too_many_arguments)]
fn initialize_file(
    ctx: &mut Context,
    receiver: Id,
    path: Id,
    selected_mode: Result<(u32, &'static str)>,
    owns: bool,
    descriptor: DescriptorLookup,
    opener: Option<Id>,
    pipe: Option<BoxedPipe>,
) -> Result<Id> {
    if let Some(raw) = state(ctx, receiver) {
        raw.close()?;
    }
    if matches!(ctx.heap.get(path), Value::Bool(_)) {
        let location = ctx.heap.current_location();
        crate::objects::runtime_warning(
            "_io",
            location.as_ref().map_or("_io", |p| p.file.as_ref()),
            location.as_ref().map_or(1, |p| p.line as isize),
            "bool is used as a file descriptor",
        )?;
    }
    let descriptor = match descriptor {
        DescriptorLookup::FromArgument => descriptor_from_argument(ctx, path),
        DescriptorLookup::Resolved(fd) => fd,
    };
    let encoded;
    let pathname = match descriptor {
        Some(fd) if fd < 0 => return Err(Error::new("ValueError", "negative file descriptor")),
        Some(_) => None,
        None => {
            let spelling = super::super::posixpath::fspath(ctx, path)?;
            let bytes: &[u8] = match ctx.heap.get(ctx.scalar_value(spelling)) {
                Value::Str(text) => text.as_bytes(),
                Value::Bytes(bytes) => bytes,
                Value::SurrogateStr(_) => {
                    encoded = ctx.encode_filesystem_string(spelling)?;
                    &encoded
                }
                _ => unreachable!("filesystem spelling"),
            };
            if bytes.contains(&0) {
                return Err(Error::new("ValueError", "embedded null byte"));
            }
            Some(bytes)
        }
    };
    // Once path conversion succeeds, a failed reinitialization drops the old stat.
    if let Some(raw) = state(ctx, receiver) {
        raw.blksize.set(super::DEFAULT_BUFFER_SIZE);
    }
    let (flags, mode) = selected_mode?;
    let (fd, blksize) = if let Some(filename) = pathname {
        if !owns {
            return Err(Error::new(
                "ValueError",
                "Cannot use closefd=False with file name",
            ));
        }
        if opener.is_some_and(|v| v != NONE) {
            return Err(Error::new(
                "TypeError",
                "custom FileIO openers are unsupported",
            ));
        }
        match crate::objects::open_raw_file(filename, flags, mode.ends_with('+')) {
            Ok((file, metadata)) => (file.into_raw_fd(), file_block_size(metadata.blksize())),
            Err(error) => return Err(ctx.with_errno_filename(error.into(), path)?),
        }
    } else {
        let fd = descriptor.unwrap();
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
        (fd, blksize)
    };
    let raw = Rc::new(RawFile {
        fd: Cell::new(Some(fd)),
        closefd: owns,
        pipe,
        mode,
        blksize: Cell::new(blksize),
        repr_active: Cell::new(false),
    });
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("FileIO is an IOBase instance")
    };
    *memory_stream = Some(MemoryStream::Raw(raw.clone()));
    let initialized = ctx.set_attr(receiver, "name", path).and_then(|()| {
        if descriptor.is_some() && flags == 2 {
            // Append seeks a passed descriptor to EOF, but does not alter its OS flags.
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
    });
    if let Err(error) = initialized {
        if descriptor.is_some() {
            // Only a successful initialization takes ownership of the supplied fd.
            raw.fd.take();
            raw.blksize.set(super::DEFAULT_BUFFER_SIZE);
        } else {
            raw.close()?;
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

fn read(ctx: &mut Context, fd: RawFd, size: i64, line: bool) -> Result<Id> {
    if size >= 0 && !line {
        let mut bytes = builtins::zeroed_bytes(size)?;
        let Some(n) = read_into(fd, &mut bytes)? else {
            return Ok(NONE);
        };
        bytes.truncate(n);
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    while size < 0 || bytes.len() < size as usize {
        let cap = if line { 1 } else { chunk.len() };
        match read_into(fd, &mut chunk[..cap])? {
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
        "write" | "readinto" => 1..=1,
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
        "closed" => return Ok(builtins::boolean(is_closed)),
        "closefd" => return Ok(builtins::boolean(raw.as_ref().is_none_or(|r| r.closefd))),
        "mode" => return Ok(ctx.text(raw.as_ref().map_or("wb", |raw| raw.mode))),
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
                raw.mode,
                if raw.closefd { "True" } else { "False" }
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
    if name == "readinto" && !matches!(ctx.heap.get(args[0]), Value::ByteArray(_)) {
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
            // SAFETY: ftruncate accepts an integer descriptor and size.
            if unsafe { libc::ftruncate(fd, size) } < 0 {
                return Err(os_error(std::io::Error::last_os_error()));
            }
            raw.blksize.set(super::DEFAULT_BUFFER_SIZE);
            Ok(ctx.int(size))
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
            read(ctx, fd, size, name == "readline")
        }
        "readinto" => {
            raw.check_direction(true)?;
            let Value::ByteArray(buf) = ctx.heap.get_mut(args[0]) else {
                unreachable!()
            };
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
