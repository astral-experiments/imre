//! Runtime-loaded Python sockets share descriptor ownership and syscalls with typed sockets.
use super::{Context, Error, Id, NONE, Result, Value, argument, option};
use crate::objects::socket as native;

const CONSTANTS: &[(&str, i32)] = &[
    ("AF_INET", libc::AF_INET),
    ("AF_INET6", libc::AF_INET6),
    ("AF_UNSPEC", libc::AF_UNSPEC),
    ("AF_UNIX", libc::AF_UNIX),
    ("AI_PASSIVE", libc::AI_PASSIVE),
    ("SOCK_STREAM", libc::SOCK_STREAM),
    ("SOL_SOCKET", libc::SOL_SOCKET),
    ("IPPROTO_TCP", libc::IPPROTO_TCP),
    ("IPPROTO_IPV6", libc::IPPROTO_IPV6),
    ("IPV6_V6ONLY", libc::IPV6_V6ONLY),
    ("SO_REUSEADDR", libc::SO_REUSEADDR),
    ("SO_SNDBUF", libc::SO_SNDBUF),
    ("TCP_NODELAY", libc::TCP_NODELAY),
    ("SHUT_RD", libc::SHUT_RD),
    ("SHUT_WR", libc::SHUT_WR),
    ("SHUT_RDWR", libc::SHUT_RDWR),
    ("SOMAXCONN", libc::SOMAXCONN),
];

/// Seed the low-level module once so normal module assignment and deletion remain observable.
pub(in super::super) fn install_lowlevel_socket(ctx: &mut Context, scope: Id) -> Result<()> {
    // socket.py imports these kinds once, so deleting its copy must not fall back to a native value.
    let kinds = [
        ("SOCK_DGRAM", libc::SOCK_DGRAM),
        ("SOCK_RAW", libc::SOCK_RAW),
        ("SOCK_RDM", libc::SOCK_RDM),
        ("SOCK_SEQPACKET", libc::SOCK_SEQPACKET),
    ];
    for (name, value) in CONSTANTS.iter().copied().chain(kinds) {
        let value = ctx.int(i64::from(value));
        ctx.bind(scope, name, value);
    }
    for name in ["has_ipv6", "socket", "error"] {
        let value = attribute(ctx, name)?;
        ctx.bind(scope, name, value);
    }
    Ok(())
}

pub(super) fn attribute(ctx: &mut Context, name: &str) -> Result<Id> {
    if let Some(&(_, value)) = CONSTANTS.iter().find(|&&(key, _)| key == name) {
        return Ok(ctx.int(i64::from(value)));
    }
    match name {
        // IPv6 is compiled in on the supported Linux and macOS targets, independently of
        // whether a particular process can create or bind an IPv6 socket.
        "has_ipv6" => Ok(super::TRUE),
        "socket" => Ok(ctx.builtin("socket.socket")),
        "getaddrinfo" | "getfqdn" | "gaierror" => {
            Ok(ctx.alloc(Value::Native(format!("socket.{name}"))))
        }
        "error" => Ok(ctx.builtin("OSError")),
        "timeout" => Ok(ctx.alloc(Value::Native("TimeoutError".into()))),
        _ => Err(Error::new(
            "AttributeError",
            format!("module 'socket' has no attribute '{name}'"),
        )),
    }
}

fn integer(ctx: &Context, id: Id) -> Result<i32> {
    i32::try_from(ctx.as_integer(id)?)
        .map_err(|_| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn check_args(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    max: usize,
    keys: &[&str],
) -> Result<()> {
    if args.len() > max || kwargs.iter().any(|(k, _)| !keys.contains(&k.as_str())) {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments for {name}"),
        ));
    }
    Ok(())
}

fn int_option(
    ctx: &Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    index: usize,
    key: &str,
    default: i32,
) -> Result<i32> {
    option(args, index, kwargs, key)?
        .map(|v| integer(ctx, v))
        .transpose()
        .map(|n| n.unwrap_or(default))
}

fn pack_ipv4(ctx: &mut Context, addr: &libc::sockaddr_in) -> Id {
    let (host, port) = native::endpoint(addr);
    let host = ctx.text(host.to_string());
    let port = ctx.int(i64::from(port));
    ctx.alloc(Value::Tuple(vec![host, port]))
}

fn pack_address(ctx: &mut Context, addr: &native::InternetAddress) -> Result<Id> {
    let fields = match addr {
        native::InternetAddress::V4(addr) => return Ok(pack_ipv4(ctx, addr)),
        native::InternetAddress::V6(addr) => {
            let host = native::ipv6_text(&addr.sin6_addr)?;
            vec![
                ctx.text(host),
                ctx.int(i64::from(u16::from_be(addr.sin6_port))),
                ctx.int(i64::from(u32::from_be(addr.sin6_flowinfo))),
                ctx.int(i64::from(addr.sin6_scope_id)),
            ]
        }
    };
    Ok(ctx.alloc(Value::Tuple(fields)))
}

fn address(
    ctx: &mut Context,
    family: i32,
    pair: Id,
    name: &str,
) -> Result<native::InternetAddress> {
    if family == libc::AF_INET6 {
        let fields = ctx.tuple_items(pair)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "{name}(): AF_INET6 address must be tuple, not {}",
                    ctx.type_name(pair)
                ),
            )
        })?;
        if !(2..=4).contains(&fields.len()) {
            return Err(Error::new(
                "TypeError",
                "AF_INET6 address must be a tuple (host, port[, flowinfo[, scopeid]])",
            ));
        }
        // Convert in CPython's argument order; __index__ may run arbitrary code.
        let host = match ctx.heap.get(ctx.scalar_value(fields[0])) {
            Value::Bytes(_) | Value::ByteArray(_) => ctx.bytes_like(fields[0])?.to_vec(),
            Value::Str(_) | Value::SurrogateStr(_) => {
                let text = ctx.str_value(fields[0])?;
                if !text.is_ascii() {
                    return Err(Error::new(
                        "NotImplementedError",
                        "IPv6 Unicode host names require IDNA encoding",
                    ));
                }
                text.as_bytes().to_vec()
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "str, bytes or bytearray expected, not {}",
                        ctx.type_name(fields[0])
                    ),
                ));
            }
        };
        if host.contains(&0) {
            return Err(Error::new(
                "TypeError",
                "host name must not contain null character",
            ));
        }
        let mut index = |id| {
            ctx.index_integer(id).map_err(|error| {
                if ctx.is_builtin_error(&error, "OverflowError") {
                    Error::new("OverflowError", format!("{name}(): port must be 0-65535."))
                } else {
                    error
                }
            })
        };
        let port = index(fields[1])?
            .to_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", format!("{name}(): port must be 0-65535."))
            })?;
        let mut unsigned = |id| index(id).map(unsigned_mask);
        let flow = fields
            .get(2)
            .copied()
            .map(&mut unsigned)
            .transpose()?
            .unwrap_or(0);
        let scope = fields
            .get(3)
            .copied()
            .map(unsigned)
            .transpose()?
            .unwrap_or(0);
        return Ok(native::ipv6_address(&host, port, flow, scope, name)?);
    }
    let Value::Tuple(fields) = ctx.heap.get(pair) else {
        return Err(Error::new(
            "TypeError",
            "AF_INET address must be a pair (host, port)",
        ));
    };
    let [host, port] = fields.as_slice() else {
        return Err(Error::new(
            "TypeError",
            "AF_INET address must be a pair (host, port)",
        ));
    };
    Ok(native::InternetAddress::V4(native::address(
        ctx.str_value(*host)?,
        ctx.as_integer(*port)?,
        name,
    )?))
}

fn unsigned_mask(value: crate::Int) -> u32 {
    use num_traits::ToPrimitive;
    (value.big().as_ref() & num_bigint::BigInt::from(u32::MAX))
        .to_u32()
        .expect("masked uint32")
}

fn getaddrinfo(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let keys = ["host", "port", "family", "type", "proto", "flags"];
    check_args("getaddrinfo", args, kwargs, keys.len(), &keys)?;
    let host =
        option(args, 0, kwargs, "host")?.ok_or_else(|| Error::new("TypeError", "missing host"))?;
    let service =
        option(args, 1, kwargs, "port")?.ok_or_else(|| Error::new("TypeError", "missing port"))?;
    let host = (host != NONE).then(|| ctx.str_value(host)).transpose()?;
    let service = match ctx.heap.get(service) {
        Value::None => None,
        Value::Str(s) => Some(s.clone()),
        Value::Int(n) => Some(n.to_string()),
        _ => {
            return Err(Error::new(
                "TypeError",
                "port must be an integer or a string",
            ));
        }
    };
    let rows = native::getaddrinfo(
        host,
        service.as_deref(),
        int_option(ctx, args, kwargs, 2, "family", libc::AF_UNSPEC)?,
        int_option(ctx, args, kwargs, 3, "type", 0)?,
        int_option(ctx, args, kwargs, 4, "proto", 0)?,
        int_option(ctx, args, kwargs, 5, "flags", 0)?,
    )?;
    let family = ctx.int(i64::from(libc::AF_INET));
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let ty = ctx.int(i64::from(row.ty));
        let protocol = ctx.int(i64::from(row.protocol));
        let name = ctx.text(row.canonical);
        let addr = pack_ipv4(ctx, &row.addr);
        result.push(ctx.alloc(Value::Tuple(vec![family, ty, protocol, name, addr])));
    }
    Ok(ctx.alloc(Value::List(result)))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "socket" => {
            let keys = ["family", "type", "proto"];
            check_args(name, args, kwargs, keys.len(), &keys)?;
            let handle = native::open(
                int_option(ctx, args, kwargs, 0, "family", -1)?,
                int_option(ctx, args, kwargs, 1, "type", -1)?,
                int_option(ctx, args, kwargs, 2, "proto", -1)?,
            )?;
            return Ok(ctx.alloc(Value::Socket(handle)));
        }
        "getaddrinfo" => return getaddrinfo(ctx, args, kwargs),
        "getfqdn" => {
            check_args(name, args, kwargs, 1, &["name"])?;
            let host = option(args, 0, kwargs, "name")?
                .map(|v| ctx.str_value(v))
                .transpose()?
                .unwrap_or("");
            return Ok(ctx.text(native::fqdn(host)?));
        }
        _ => {}
    }
    let Some(method) = name.strip_prefix("socket.") else {
        return Err(Error::new(
            "TypeError",
            format!("'{name}' is not a socket operation"),
        ));
    };
    let (receiver, args) = args
        .split_first()
        .ok_or_else(|| Error::new("TypeError", "missing socket receiver"))?;
    let Value::Socket(handle) = ctx.heap.get(*receiver) else {
        return Err(Error::new("TypeError", "expected a socket"));
    };
    let s = handle.clone();
    if method == "makefile" {
        check_args(method, args, kwargs, 2, &["mode", "buffering"])?;
        let binary_reader = if let Some(mode) = option(args, 0, kwargs, "mode")? {
            let storage = ctx
                .string_storage(mode)
                .ok_or_else(|| Error::new("TypeError", "expected str"))?;
            match ctx.heap.get(storage) {
                Value::Str(text) if text.bytes().all(|c| matches!(c, b'r' | b'w' | b'b')) => {
                    text == "rb"
                }
                _ => {
                    let repr = ctx.repr(mode);
                    return Err(Error::new(
                        "ValueError",
                        format!("invalid mode {repr} (only r, w, b allowed)"),
                    ));
                }
            }
        } else {
            false
        };
        let buffering = int_option(ctx, args, kwargs, 1, "buffering", -1)?;
        if !binary_reader || buffering != -1 {
            return Err(Error::new(
                "NotImplementedError",
                "socket.makefile supports the default buffered binary reader",
            ));
        }
        let reader = super::ReadSource::Socket(native::SocketInput::try_new(s)?);
        return Ok(ctx.alloc(super::BinaryFile::reader(reader).into_value()?));
    }
    let max = match method {
        "bind" | "connect" | "listen" | "setblocking" | "shutdown" => 1,
        "recv" | "send" | "sendall" => 2,
        "setsockopt" | "__exit__" | "recv_into" => 3,
        _ => 0,
    };
    check_args(method, args, kwargs, max, &[])?;
    match method {
        "__enter__" => return Ok(*receiver),
        "__exit__" | "close" => native::close(&s)?,
        "fileno" => return Ok(ctx.int(i64::from(native::fd(&s)))),
        "bind" => native::bind(
            &s,
            &address(ctx, native::family(&s), argument(args, 0)?, method)?,
        )?,
        "connect" => native::connect_address(
            &s,
            &address(ctx, native::family(&s), argument(args, 0)?, method)?,
        )?,
        "listen" => native::listen(
            &s,
            int_option(ctx, args, kwargs, 0, "backlog", libc::SOMAXCONN.min(128))?,
        )?,
        "accept" => {
            let (child, peer) = native::accept(&s)?;
            let child = ctx.alloc(Value::Socket(child));
            let peer = pack_address(ctx, &peer)?;
            return Ok(ctx.alloc(Value::Tuple(vec![child, peer])));
        }
        "getsockname" => return pack_address(ctx, &native::sockname(&s)?),
        "setblocking" => native::set_blocking_mode(&s, ctx.as_integer(argument(args, 0)?)? != 0)?,
        "getblocking" => {
            return Ok(if native::imre_socket_getblocking(&s) {
                super::TRUE
            } else {
                super::FALSE
            });
        }
        "setsockopt" => native::set_option(
            &s,
            integer(ctx, argument(args, 0)?)?,
            integer(ctx, argument(args, 1)?)?,
            integer(ctx, argument(args, 2)?)?,
        )?,
        "shutdown" => native::shutdown(&s, integer(ctx, argument(args, 0)?)?)?,
        "recv_into" => {
            let buf = argument(args, 0)?;
            let n = args
                .get(1)
                .map(|&id| ctx.as_integer(id))
                .transpose()?
                .unwrap_or(0);
            let n = usize::try_from(n)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recv_into"))?;
            let flags = int_option(ctx, args, kwargs, 2, "flags", 0)?;
            let Value::ByteArray(bytes) = ctx.heap.get_mut(buf) else {
                return Err(Error::new(
                    "TypeError",
                    "recv_into() requires a writable bytes-like object",
                ));
            };
            let n = if n == 0 { bytes.len() } else { n };
            let target = bytes
                .get_mut(..n)
                .ok_or_else(|| Error::new("ValueError", "buffer too small for requested bytes"))?;
            let count = native::recv_into(&s, target, flags)?;
            return Ok(ctx.int(count as i64));
        }
        "recv" => {
            let n = usize::try_from(ctx.as_integer(argument(args, 0)?)?)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recv"))?;
            let bytes = native::recv(&s, n, int_option(ctx, args, kwargs, 1, "flags", 0)?)?;
            return Ok(ctx.alloc(Value::Bytes(bytes)));
        }
        "send" | "sendall" => {
            let (Value::Bytes(bytes) | Value::ByteArray(bytes)) = ctx.heap.get(argument(args, 0)?)
            else {
                return Err(Error::new("TypeError", "a bytes-like object is required"));
            };
            let flags = int_option(ctx, args, kwargs, 1, "flags", 0)?;
            if method == "send" {
                return Ok(ctx.int(native::send(native::fd(&s), bytes, flags)? as i64));
            }
            native::send_all(&s, bytes, flags)?;
        }
        _ => {
            return Err(Error::new(
                "AttributeError",
                format!("socket has no attribute '{method}'"),
            ));
        }
    }
    Ok(NONE)
}
