use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, RawFd};
use std::process::{Command, Stdio};

use super::{
    Context, Error, FALSE, Id, NONE, Result, TRUE, Value, argument, instance, option, os_error,
};

/// Binary handles retain their access mode after close.
pub(in crate::dynamic) enum BinaryFile {
    Descriptor(super::io::BufferedFd),
    // The original sys stream owns its encoder and the shared process binary buffer.
    Standard {
        file: crate::objects::TextFile,
        path: Id,
    },
    Reader {
        slot: RefCell<Option<ReadSource>>,
        path: Option<Id>,
    },
    Writer {
        file: RefCell<crate::objects::FileState>,
        path: Id,
        mode: &'static str,
    },
}

impl BinaryFile {
    /// Opens the documented read, truncate, append, or exclusive-create mode.
    pub(in crate::dynamic) fn open(
        path: &str,
        name: Id,
        mode: &str,
        writer_capacity: Option<usize>,
    ) -> Result<Self> {
        let (flags, mode) = match mode {
            "rb" => {
                let (file, _) = crate::objects::open_file(path, 0)?;
                return Ok(Self::reader(
                    ReadSource::File(BufReader::new(file)),
                    Some(name),
                ));
            }
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
        Ok(Self::Writer {
            file: RefCell::new(crate::objects::FileState::binary_writer(
                path,
                flags,
                writer_capacity,
            )?),
            path: name,
            mode,
        })
    }

    /// Files and sockets share the same buffered reader methods.
    pub(in crate::dynamic) fn reader(reader: ReadSource, path: Option<Id>) -> Self {
        Self::Reader {
            path,
            slot: RefCell::new(Some(reader)),
        }
    }

    /// Filename objects and binary access modes remain observable after close.
    pub(in crate::dynamic) fn path_metadata(&self) -> Option<(Id, &'static str)> {
        match self {
            Self::Reader {
                path: Some(path), ..
            } => Some((*path, "rb")),
            Self::Writer { path, mode, .. } => Some((*path, mode)),
            Self::Standard { path, .. } => Some((*path, "wb")),
            _ => None,
        }
    }

    /// A refused handle allocation drops its reader and releases any retained socket.
    pub(in crate::dynamic) fn into_value(self) -> Result<Value> {
        crate::shared::Shared::try_new(self)
            .map(Value::File)
            .map_err(|_| Error::new("MemoryError", ""))
    }

    /// SocketIO reports its live descriptor, or -1 once this reader has closed.
    pub(in crate::dynamic) fn socket_reader_name(&self) -> Option<RawFd> {
        match self {
            Self::Reader { slot, path: None } => {
                Some(slot.borrow().as_ref().map_or(-1, AsRawFd::as_raw_fd))
            }
            _ => None,
        }
    }

    /// All aliases observe closure through the same buffered stream.
    pub(in crate::dynamic) fn closed(&self) -> bool {
        match self {
            Self::Descriptor(fd) => fd.closed(),
            Self::Standard { file, .. } => file.closed(),
            Self::Reader { slot, .. } => slot.borrow().is_none(),
            Self::Writer { file, .. } => file.borrow().closed(),
        }
    }
}

/// One buffered cursor serves filesystem input and a socket retained by makefile.
pub(in crate::dynamic) enum ReadSource {
    File(BufReader<std::fs::File>),
    Socket(crate::objects::socket::SocketInput),
}

impl ReadSource {
    fn seekable(&mut self) -> bool {
        match self {
            Self::File(file) => file.get_mut().stream_position().is_ok(),
            Self::Socket(_) => false,
        }
    }
}

impl Read for ReadSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Socket(sock) => sock.read(buf),
        }
    }
}

impl BufRead for ReadSource {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        match self {
            Self::File(file) => file.fill_buf(),
            Self::Socket(sock) => sock.fill_buf(),
        }
    }

    fn consume(&mut self, count: usize) {
        match self {
            Self::File(file) => file.consume(count),
            Self::Socket(sock) => sock.consume(count),
        }
    }
}

impl Seek for ReadSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(pos),
            Self::Socket(_) => Err(std::io::Error::from_raw_os_error(libc::ESPIPE)),
        }
    }
}

impl AsRawFd for ReadSource {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::File(file) => file.get_ref().as_raw_fd(),
            Self::Socket(sock) => sock.as_raw_fd(),
        }
    }
}

fn dirname(path: &str) -> &str {
    let head = path.rfind('/').map_or("", |i| &path[..=i]);
    if head.trim_start_matches('/').is_empty() {
        head
    } else {
        head.trim_end_matches('/')
    }
}

fn run_process(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let command = argument(args, 0)?;
    let items = ctx.items(command)?;
    let argv = items
        .iter()
        .map(|&id| ctx.str_value(id))
        .collect::<Result<Vec<_>>>()?;
    let (executable, rest) = argv
        .split_first()
        .ok_or_else(|| Error::new("ValueError", "empty subprocess argument list"))?;
    let named = |name| {
        kwargs
            .iter()
            .find(|(key, _)| key == name)
            .map(|&(_, id)| id)
    };
    let capture = named("capture_output") == Some(TRUE);
    let piped = |name| -> Result<bool> {
        match named(name) {
            None | Some(NONE) => Ok(capture),
            Some(_) if capture => Err(Error::new(
                "ValueError",
                "stdout and stderr arguments may not be used with capture_output",
            )),
            Some(id) if ctx.as_integer(id)? == -1 => Ok(true),
            _ => Err(Error::new("ValueError", "expected subprocess.PIPE")),
        }
    };
    let stdout = piped("stdout")?;
    let stderr = piped("stderr")?;
    let stdio = |captured| {
        if captured {
            Stdio::piped()
        } else {
            Stdio::inherit()
        }
    };
    let output = Command::new(executable)
        .args(rest)
        .stdout(stdio(stdout))
        .stderr(stdio(stderr))
        .output()
        .map_err(os_error)?;
    let exit_code = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        -output.status.signal().unwrap_or(1)
    });
    if named("check") == Some(TRUE) && exit_code != 0 {
        return Err(Error::new(
            "CalledProcessError",
            format!("subprocess exited with status {exit_code}"),
        ));
    }
    let text = named("text") == Some(TRUE);
    let mut stream = |captured: bool, bytes: Vec<u8>| -> Result<Id> {
        if !captured {
            return Ok(NONE);
        }
        if text {
            let text = String::from_utf8(bytes)
                .map_err(|err| Error::new("UnicodeDecodeError", err.to_string()))?;
            Ok(ctx.text(text.replace("\r\n", "\n").replace('\r', "\n")))
        } else {
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
    };
    let stdout = stream(stdout, output.stdout)?;
    let stderr = stream(stderr, output.stderr)?;
    let status = ctx.int(i64::from(exit_code));
    Ok(instance(
        ctx,
        "subprocess.CompletedProcess",
        &[
            ("args", command),
            ("returncode", status),
            ("stdout", stdout),
            ("stderr", stderr),
        ],
    ))
}

pub(super) fn read_length(ctx: &mut Context, size: Option<Id>) -> Result<i64> {
    let Some(id) = size.filter(|&id| id != NONE) else {
        return Ok(-1);
    };
    let integer = ctx.index_value(id)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "argument should be integer or None, not '{}'",
                ctx.type_name(id)
            ),
        )
    })?;
    integer.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!(
                "cannot fit '{}' into an index-sized integer",
                ctx.type_name(id)
            ),
        )
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let first = argument(args, 0)?;
    match name {
        "os.path.dirname" => return Ok(ctx.text(dirname(ctx.str_value(first)?).to_owned())),
        "subprocess.run" => return run_process(ctx, args, kwargs),
        _ => {}
    }
    let Value::File(handle) = ctx.heap.get(first) else {
        return Err(Error::new("TypeError", "expected a file"));
    };
    if name == "file.readlines" {
        return super::io::lines(ctx, first, "readlines", &args[1..], kwargs);
    }
    let handle = handle.clone();
    let (slot, socket) = match handle.as_ref() {
        BinaryFile::Descriptor(file) => return file.call(ctx, first, name, &args[1..], kwargs),
        BinaryFile::Standard { file, .. } => {
            return standard_buffer_method(ctx, file, first, name, &args[1..], kwargs);
        }
        BinaryFile::Reader { slot, path } => (slot, path.is_none()),
        BinaryFile::Writer { file, .. } => {
            if name == "file.seek" {
                let value = option(args, 2, kwargs, "whence")?;
                let whence = value
                    .map(|v| ctx.index_integer(v))
                    .transpose()?
                    .map_or(Some(0), |v| v.to_i64().and_then(|n| i32::try_from(n).ok()))
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?;
                let offset = ctx.index_integer(argument(args, 1)?)?;
                let pos = file.borrow_mut().seek_disk(&offset, i64::from(whence))?;
                return Ok(ctx.int(pos as i64));
            }
            // Converting a size can close this same writer.
            match (name, &args[1..]) {
                ("file.read", [size]) => {
                    crate::objects::FileState::check_writer_read_size(&ctx.index_integer(*size)?)?;
                }
                ("file.readline", [size]) if read_length(ctx, Some(*size))? == 0 => {
                    return Ok(ctx.alloc(Value::Bytes(Vec::new())));
                }
                _ => {}
            }
            let mut file = file.borrow_mut();
            return match (name, &args[1..]) {
                ("file.write", [value]) => {
                    let bytes = ctx.bytes_like(*value)?;
                    file.write_bytes(bytes)?;
                    Ok(ctx.int(bytes.len() as i64))
                }
                ("file.flush", []) => {
                    file.flush()?;
                    Ok(NONE)
                }
                ("file.close", []) | ("file.__exit__", [_, _, _]) => {
                    file.close().map_err(crate::objects::errno_error)?;
                    Ok(NONE)
                }
                ("file.__enter__", []) if file.closed() => {
                    Err(Error::new("ValueError", "I/O operation on closed file."))
                }
                ("file.__enter__", []) => Ok(first),
                ("file.fileno", []) => Ok(ctx.int(i64::from(file.fileno()?))),
                ("file.tell", []) => Ok(ctx.int(file.tell()? as i64)),
                ("file.readable" | "file.writable" | "file.seekable" | "file.isatty", []) => {
                    let fd = file.fileno()?;
                    let enabled = match name {
                        "file.writable" => true,
                        "file.seekable" => file.tell().is_ok(),
                        // SAFETY: the borrowed writer keeps its file descriptor open.
                        "file.isatty" => unsafe { libc::isatty(fd) != 0 },
                        _ => false,
                    };
                    Ok(crate::dynamic::builtins::boolean(enabled))
                }
                ("file.read" | "file.readline", [] | [_]) | ("file.readinto", [_]) => {
                    if let ("file.readinto", [buffer]) = (name, &args[1..])
                        && !matches!(ctx.heap.get(*buffer), Value::ByteArray(_))
                    {
                        return Err(Error::new(
                            "TypeError",
                            "readinto() requires a writable bytes-like object",
                        ));
                    }
                    Err(Error::new("UnsupportedOperation", "read"))
                }
                _ => Err(Error::new(
                    "TypeError",
                    format!("invalid arguments to {name}"),
                )),
            };
        }
    };
    // A Python size conversion may read from or close this same handle.
    let length = match name {
        "file.read" | "file.readline" => Some(read_length(ctx, args.get(1).copied())?),
        _ => None,
    };
    if socket && name == "file.tell" {
        // SocketIO.tell() calls its unsupported raw seek even after close.
        return Err(Error::new("UnsupportedOperation", "seek"));
    }
    if name == "file.seek" {
        let target = argument(args, 1)?;
        let value = option(args, 2, kwargs, "whence")?;
        let whence = buffered_whence(ctx, value)?;
        {
            let mut slot = slot.borrow_mut();
            let reader = slot
                .as_mut()
                .ok_or_else(|| Error::new("ValueError", "seek of closed file"))?;
            if !reader.seekable() {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
        }
        // An offset conversion may advance or close this same stream.
        let offset = buffered_offset(ctx, target)?;
        let mut slot = slot.borrow_mut();
        let reader = slot
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "I/O operation on closed file"))?;
        if whence == libc::SEEK_SET && offset < 0 {
            let ReadSource::File(file) = reader else {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            };
            // A seekable device may accept a negative absolute offset. Ask the
            // underlying file without discarding unread buffered data on failure.
            // SAFETY: the borrowed file keeps its descriptor open for the call.
            let position = unsafe {
                libc::lseek(
                    file.get_ref().as_raw_fd(),
                    offset as libc::off_t,
                    libc::SEEK_SET,
                )
            };
            if position < 0 {
                return Err(os_error(std::io::Error::last_os_error()));
            }
            let remaining = file.buffer().len();
            file.consume(remaining);
            return Ok(ctx.int(position as i64));
        }
        let pos = match whence {
            0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(Error::new("ValueError", "invalid seek whence")),
        };
        return Ok(ctx.int(reader.seek(pos).map_err(os_error)? as i64));
    }
    let mut slot = slot.borrow_mut();
    if name == "file.write" {
        return Err(Error::new("UnsupportedOperation", "write"));
    }
    if matches!(name, "file.__exit__" | "file.close") {
        if let Some(ReadSource::Socket(mut raw)) = slot.take() {
            raw.close().map_err(os_error)?;
        }
        return Ok(NONE);
    }
    let reader = slot.as_mut().ok_or_else(|| {
        let message = match name {
            "file.read" => "read of closed file",
            "file.readline" => "readline of closed file",
            "file.readinto" => "readinto of closed file",
            "file.__enter__" | "file.flush" => "I/O operation on closed file.",
            _ => "I/O operation on closed file",
        };
        Error::new("ValueError", message)
    });
    if let Some(length) = length {
        if name == "file.read" && length < -1 {
            return Err(Error::new(
                "ValueError",
                "read length must be non-negative or -1",
            ));
        }
        let mut bytes = Vec::new();
        let mut input = reader?.take(u64::try_from(length).unwrap_or(u64::MAX));
        let requested = if name == "file.read" {
            usize::try_from(length).ok()
        } else {
            None
        };
        if let Some(size) = requested {
            if size > crate::MAX_BYTES_LEN {
                return Err(Error::new("OverflowError", "byte string is too large"));
            }
            bytes
                .try_reserve_exact(size)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        if name == "file.readline" {
            input.read_until(b'\n', &mut bytes)
        } else {
            input.read_to_end(&mut bytes)
        }
        .map_err(os_error)?;
        if requested.is_some_and(|size| bytes.len() < size) {
            crate::compact_bytes(&mut bytes);
        }
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let f = reader?;
    match name {
        "file.__enter__" => Ok(first),
        "file.readable" => Ok(TRUE),
        "file.writable" => Ok(FALSE),
        "file.seekable" => Ok(crate::dynamic::builtins::boolean(f.seekable())),
        "file.isatty" => {
            use std::io::IsTerminal;
            Ok(
                if matches!(f, ReadSource::File(file) if file.get_ref().is_terminal()) {
                    TRUE
                } else {
                    FALSE
                },
            )
        }
        "file.flush" => Ok(NONE),
        "file.fileno" => Ok(ctx.int(i64::from(f.as_raw_fd()))),
        "file.tell" => Ok(ctx.int(f.stream_position().map_err(os_error)? as i64)),
        "file.readinto" => {
            let Value::ByteArray(buffer) = ctx.heap.get_mut(argument(args, 1)?) else {
                return Err(Error::new(
                    "TypeError",
                    "readinto() requires a writable bytes-like object",
                ));
            };
            let mut count = 0;
            while count < buffer.len() {
                match f.read(&mut buffer[count..]) {
                    Ok(0) => break,
                    Ok(n) => count += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(os_error(e)),
                }
            }
            Ok(ctx.int(count as i64))
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown native file method {name}"),
        )),
    }
}

fn buffered_whence(ctx: &mut Context, value: Option<Id>) -> Result<i32> {
    let whence = match value {
        None => 0,
        Some(value) => ctx
            .index_integer(value)?
            .to_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })?,
    };
    if !matches!(
        whence,
        libc::SEEK_SET | libc::SEEK_CUR | libc::SEEK_END | libc::SEEK_DATA | libc::SEEK_HOLE
    ) {
        return Err(Error::new(
            "ValueError",
            format!("whence value {whence} unsupported"),
        ));
    }
    Ok(whence)
}

fn buffered_offset(ctx: &mut Context, target: Id) -> Result<i64> {
    ctx.index_integer(target)?.to_i64().ok_or_else(|| {
        Error::new(
            "ValueError",
            format!(
                "cannot fit '{}' into an offset-sized integer",
                ctx.type_name(target)
            ),
        )
    })
}

fn standard_buffer_method(
    ctx: &mut Context,
    file: &crate::objects::TextFile,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(
        (name, args),
        ("file.close", []) | ("file.__exit__", [_, _, _])
    ) {
        if !file.closed() {
            if file.standard_writer()? {
                ctx.check_thread_reporter_mutable()?;
            }
            file.close_standard_buffer()?;
        }
        return Ok(NONE);
    }
    match (name, args) {
        ("file.write", [value]) => {
            let data = ctx.bytes_like(*value)?;
            if file.closed() {
                return Err(Error::new("ValueError", "write to closed file"));
            }
            crate::sys::write_standard_buffer(file.standard_writer()?, data)?;
            Ok(ctx.int(data.len() as i64))
        }
        ("file.flush", []) => {
            if file.closed() {
                return Err(Error::new("ValueError", "flush of closed file"));
            }
            crate::sys::flush_standard_buffer(file.standard_writer()?).map_err(os_error)?;
            Ok(NONE)
        }
        ("file.tell" | "file.seekable", []) => {
            let position = crate::sys::standard_buffer_position(file.standard_writer()?, None);
            if name == "file.seekable" {
                Ok(crate::dynamic::builtins::boolean(position.is_ok()))
            } else {
                Ok(ctx.int(position?))
            }
        }
        ("file.seek", [target] | [target, _]) => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "BufferedWriter.seek() takes no keyword arguments",
                ));
            }
            let whence = buffered_whence(ctx, args.get(1).copied())?;
            if file.closed() {
                return Err(Error::new("ValueError", "seek of closed file"));
            }
            if crate::sys::standard_buffer_position(file.standard_writer()?, None).is_err() {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            let offset = buffered_offset(ctx, *target)?;
            let pos = crate::sys::standard_buffer_position(
                file.standard_writer()?,
                Some((offset, whence)),
            )?;
            Ok(ctx.int(pos))
        }
        ("file.fileno", []) => Ok(ctx.int(if file.standard_writer()? { 2 } else { 1 })),
        ("file.writable" | "file.readable", []) => {
            file.standard_writer()?;
            Ok(crate::dynamic::builtins::boolean(name == "file.writable"))
        }
        ("file.isatty", []) => Ok(crate::dynamic::builtins::boolean(file.isatty()?)),
        ("file.__enter__", []) => {
            file.standard_writer()?;
            Ok(receiver)
        }
        ("file.read" | "file.readline", [] | [_]) | ("file.readinto", [_]) => {
            Err(Error::new("UnsupportedOperation", "read"))
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}

pub(super) fn text_method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected a text file"));
    };
    let Value::TextFile(file) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a text file"));
    };
    if !kwargs.is_empty() {
        let class = if name == "__exit__" {
            ""
        } else {
            "TextIOWrapper."
        };
        return Err(Error::new(
            "TypeError",
            format!("{class}{name}() takes no keyword arguments"),
        ));
    }
    let arity = usize::from(name == "write");
    if name != "__exit__" && args.len() != arity {
        let expected = if arity == 0 {
            "no arguments"
        } else {
            "exactly one argument"
        };
        return Err(Error::new(
            "TypeError",
            format!(
                "TextIOWrapper.{name}() takes {expected} ({} given)",
                args.len()
            ),
        ));
    }
    match name {
        "closed" => Ok(crate::dynamic::builtins::boolean(file.closed())),
        "read" | "readline" => Ok(ctx.text(file.read(name == "readline")?.into_string())),
        "write" => {
            let input = args[0];
            let count = match ctx.heap.get(ctx.scalar_value(input)) {
                Value::Str(text) => {
                    file.write(text)?;
                    text.chars().count()
                }
                Value::SurrogateStr(points) => {
                    let count = points.len();
                    let newline = points.iter().any(|&p| matches!(p, 10 | 13));
                    let stderr = file.standard_writer()?;
                    let fd = if stderr { 2 } else { 1 };
                    let encoding = ctx.text(crate::sys::standard_write_encoding(stderr));
                    ctx.heap.keep(encoding);
                    let errors = ctx.text(crate::sys::standard_field(fd, true).as_str());
                    ctx.heap.keep(errors);
                    let bytes =
                        super::codecs::encode_text(ctx, input, Some(encoding), Some(errors))?;
                    crate::sys::write_standard_encoded(stderr, &bytes, newline)?;
                    count
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("write() argument must be str, not {}", ctx.type_name(input)),
                    ));
                }
            };
            Ok(ctx.int(count as i64))
        }
        "fileno" => Ok(ctx.int(i64::from(file.fileno()?))),
        "isatty" => Ok(crate::dynamic::builtins::boolean(file.isatty()?)),
        "flush" => {
            file.flush()?;
            Ok(NONE)
        }
        "close" | "__exit__" => {
            if ctx.threads.default_stderr == Some(receiver) {
                ctx.check_thread_reporter_mutable()?;
            }
            file.close()?;
            Ok(NONE)
        }
        "__enter__" if file.closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "__enter__" => Ok(receiver),
        _ => Err(Error::new("AttributeError", name)),
    }
}
