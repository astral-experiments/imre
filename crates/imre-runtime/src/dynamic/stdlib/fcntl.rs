//! Linux fcntl commands taking integer arguments, including pipe capacity.
use super::{Context, Error, Id, NONE, Result, Value, audit, os_error, select};
use num_traits::ToPrimitive;
use std::io;

const COMMANDS: &[(&str, libc::c_int)] = &[
    ("F_DUPFD", libc::F_DUPFD),
    ("F_DUPFD_CLOEXEC", libc::F_DUPFD_CLOEXEC),
    ("F_GETFD", libc::F_GETFD),
    ("F_SETFD", libc::F_SETFD),
    ("F_GETFL", libc::F_GETFL),
    ("F_SETFL", libc::F_SETFL),
    ("F_GETPIPE_SZ", libc::F_GETPIPE_SZ),
    ("F_SETPIPE_SZ", libc::F_SETPIPE_SZ),
];

pub(super) fn constant(name: &str) -> Option<i32> {
    COMMANDS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|&(_, n)| n)
}

fn syscall(fd: i32, cmd: i32, arg: libc::c_ulong) -> Result<i32> {
    // Only scalar commands reach this call: none reads or writes through arg.
    loop {
        // SAFETY: fcntl validates the descriptor, and arg is a native-size scalar.
        let result = unsafe { libc::fcntl(fd, cmd, arg) };
        if result >= 0 {
            return Ok(result);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(os_error(error));
        }
    }
}

pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "fcntl.fcntl() takes no keyword arguments",
        ));
    }
    if !(2..=3).contains(&args.len()) {
        let bound = if args.len() < 2 {
            "at least 2"
        } else {
            "at most 3"
        };
        return Err(Error::new(
            "TypeError",
            format!("fcntl expected {bound} arguments, got {}", args.len()),
        ));
    }
    let fd = select::fd(ctx, args[0])?;
    let cmd = ctx
        .index_integer(args[1])?
        .to_i64()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    let fd_obj = ctx.int(i64::from(fd));
    let cmd_obj = ctx.int(i64::from(cmd));
    audit::event(
        ctx,
        "fcntl.fcntl",
        &[fd_obj, cmd_obj, args.get(2).copied().unwrap_or(NONE)],
    )?;
    if !COMMANDS.iter().any(|&(_, value)| value == cmd) {
        return Err(Error::new(
            "NotImplementedError",
            "unsupported fcntl command",
        ));
    }
    let Some(&arg) = args.get(2) else {
        return Ok(ctx.int(i64::from(syscall(fd, cmd, 0)?)));
    };
    if let Some((_, n)) = ctx.index_result(arg)? {
        // CPython's unsigned-int converter masks arbitrarily large and negative indices.
        let low = (&*n.big() & num_bigint::BigInt::from(u32::MAX))
            .to_u32()
            .unwrap();
        return Ok(ctx.int(i64::from(syscall(fd, cmd, libc::c_ulong::from(low))?)));
    }
    let encoded;
    let bytes = if ctx.string_storage(arg).is_some() {
        encoded = ctx.encode_utf8_codepoints(arg, false, "strict")?;
        encoded.as_slice()
    } else {
        ctx.check_buffer_exporter(arg)?;
        match ctx.heap.get(ctx.scalar_value(arg)) {
            Value::Array(array) => array.as_bytes(),
            Value::Bytes(_) | Value::ByteArray(_) | Value::MemoryView { .. } => {
                ctx.bytes_like(arg)?
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "fcntl() argument 3 must be an integer, a bytes-like object, or a string, not {}",
                        ctx.type_name(arg)
                    ),
                ));
            }
        }
    };
    if bytes.len() > 1024 {
        return Err(Error::new("ValueError", "fcntl argument 3 is too long"));
    }
    // Match CPython's temporary buffer even for scalar commands. Query commands
    // ignore the argument, and the caller receives a fresh copy of the input bytes.
    let mut buffer = [0u8; 1024];
    let len = bytes.len();
    buffer[..len].copy_from_slice(bytes);
    syscall(fd, cmd, buffer.as_ptr() as libc::c_ulong)?;
    Ok(ctx.alloc(Value::Bytes(buffer[..len].to_vec())))
}
