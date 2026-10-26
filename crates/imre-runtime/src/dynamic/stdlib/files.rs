use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, RawFd};
use std::process::{Command, Stdio};

use super::{
    Context, Error, FALSE, Id, NONE, Result, TRUE, Value, argument, instance, option, os_error,
};

/// Binary handles retain their access mode after close.
pub(in crate::dynamic) enum BinaryFile {
    Reader {
        slot: RefCell<Option<ReadSource>>,
        socket: bool,
    },
    Writer(RefCell<crate::objects::FileState>),
}

impl BinaryFile {
    /// Opens the documented read, truncate, append, or exclusive-create mode.
    pub(in crate::dynamic) fn open(path: &str, mode: &str) -> Result<Self> {
        let mode = match mode {
            "rb" => {
                let (file, _) = crate::objects::open_file(path, 0)?;
                return Ok(Self::reader(ReadSource::File(BufReader::new(file))));
            }
            "wb" => 1,
            "ab" => 2,
            "xb" => 3,
            _ => {
                return Err(Error::new(
                    "ValueError",
                    "unsupported mode for dynamic files",
                ));
            }
        };
        Ok(Self::Writer(RefCell::new(
            crate::objects::FileState::binary_writer(path, mode)?,
        )))
    }

    /// Files and sockets share the same buffered reader methods.
    pub(in crate::dynamic) fn reader(reader: ReadSource) -> Self {
        Self::Reader {
            socket: matches!(reader, ReadSource::Socket(_)),
            slot: RefCell::new(Some(reader)),
        }
    }

    /// A refused handle allocation drops its reader and releases any retained socket.
    pub(in crate::dynamic) fn into_value(self) -> Result<Value> {
        crate::shared::Shared::try_new(self)
            .map(Value::File)
            .map_err(|_| Error::new("MemoryError", ""))
    }

    /// All aliases observe closure through the same buffered stream.
    pub(in crate::dynamic) fn closed(&self) -> bool {
        match self {
            Self::Reader { slot, .. } => slot.borrow().is_none(),
            Self::Writer(file) => file.borrow().closed(),
        }
    }
}

/// One buffered cursor serves filesystem input and a socket retained by makefile.
pub(in crate::dynamic) enum ReadSource {
    File(BufReader<std::fs::File>),
    Socket(crate::objects::socket::SocketInput),
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

fn join(left: &str, right: &str) -> String {
    if right.starts_with('/') || left.is_empty() {
        return right.to_owned();
    }
    if left.ends_with('/') {
        format!("{left}{right}")
    } else {
        format!("{left}/{right}")
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

fn abspath(path: &str) -> std::io::Result<String> {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        let cwd = std::env::current_dir()?;
        join(
            cwd.to_str()
                .ok_or_else(|| std::io::Error::other("non-UTF-8 working directory"))?,
            path,
        )
    };
    let prefix = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Ok(format!("{prefix}{}", parts.join("/")))
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

fn read_length(ctx: &mut Context, size: Option<Id>) -> Result<i64> {
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
        "os.path.join" => {
            let mut path = ctx.str_value(first)?.to_owned();
            for &part in &args[1..] {
                path = join(&path, ctx.str_value(part)?);
            }
            return Ok(ctx.text(path));
        }
        "os.path.abspath" => return Ok(ctx.text(abspath(ctx.str_value(first)?).map_err(os_error)?)),
        "os.path.dirname" => return Ok(ctx.text(dirname(ctx.str_value(first)?).to_owned())),
        "subprocess.run" => return run_process(ctx, args, kwargs),
        _ => {}
    }
    let Value::File(handle) = ctx.heap.get(first) else {
        return Err(Error::new("TypeError", "expected a file"));
    };
    let handle = handle.clone();
    let (slot, socket) = match handle.as_ref() {
        BinaryFile::Reader { slot, socket } => (slot, *socket),
        BinaryFile::Writer(file) => {
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
    // BufferedReader validates whence before checking the stream or converting the offset.
    let whence = if name == "file.seek" {
        let value = option(args, 2, kwargs, "whence")?;
        let value = match value {
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
            value,
            libc::SEEK_SET | libc::SEEK_CUR | libc::SEEK_END | libc::SEEK_DATA | libc::SEEK_HOLE
        ) {
            return Err(Error::new(
                "ValueError",
                format!("whence value {value} unsupported"),
            ));
        }
        value
    } else {
        0
    };
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
            "file.seek" => "seek of closed file",
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
            bytes.shrink_to_fit();
        }
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let f = reader?;
    match name {
        "file.__enter__" => Ok(first),
        "file.flush" => Ok(NONE),
        "file.fileno" => Ok(ctx.int(i64::from(f.as_raw_fd()))),
        "file.tell" => Ok(ctx.int(f.stream_position().map_err(os_error)? as i64)),
        "file.seek" => {
            let target = argument(args, 1)?;
            if socket {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            let offset = ctx.as_integer(target)?;
            let pos = match whence {
                0 => SeekFrom::Start(
                    u64::try_from(offset)
                        .map_err(|_| Error::new("OSError", "invalid negative seek position"))?,
                ),
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return Err(Error::new("ValueError", "invalid seek whence")),
            };
            Ok(ctx.int(f.seek(pos).map_err(os_error)? as i64))
        }
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
        "read" | "readline" => Ok(ctx.text(file.read(name == "readline")?.into_string())),
        "write" => {
            let Value::Str(value) = ctx.heap.get(args[0]) else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "write() argument must be str, not {}",
                        ctx.type_name(args[0])
                    ),
                ));
            };
            file.write(value)?;
            Ok(ctx.int(value.chars().count() as i64))
        }
        "isatty" => Ok(if file.isatty()? { TRUE } else { FALSE }),
        "flush" => {
            file.flush()?;
            Ok(NONE)
        }
        "close" | "__exit__" => {
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
