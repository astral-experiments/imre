//! Runtime-loaded Python sockets share descriptor ownership and syscalls with typed sockets.
use super::{Context, Error, Id, NONE, Result, Value, argument, option};
use crate::objects::socket as native;

pub(super) fn attribute(ctx: &mut Context, name: &str) -> Result<Id> {
    let value = match name {
        "AF_INET" => libc::AF_INET,
        "AF_UNSPEC" => libc::AF_UNSPEC,
        "AF_UNIX" => libc::AF_UNIX,
        "SOCK_STREAM" => libc::SOCK_STREAM,
        "SOL_SOCKET" => libc::SOL_SOCKET,
        "IPPROTO_TCP" => libc::IPPROTO_TCP,
        "SO_REUSEADDR" => libc::SO_REUSEADDR,
        "SO_SNDBUF" => libc::SO_SNDBUF,
        "TCP_NODELAY" => libc::TCP_NODELAY,
        "SHUT_RD" => libc::SHUT_RD,
        "SHUT_WR" => libc::SHUT_WR,
        "SHUT_RDWR" => libc::SHUT_RDWR,
        "SOMAXCONN" => libc::SOMAXCONN,
        "socket" | "getaddrinfo" | "getfqdn" | "gaierror" => {
            return Ok(ctx.alloc(Value::Native(format!("socket.{name}"))));
        }
        "error" => return Ok(ctx.alloc(Value::Native("OSError".into()))),
        "timeout" => return Ok(ctx.alloc(Value::Native("TimeoutError".into()))),
        _ => {
            return Err(Error::new(
                "AttributeError",
                format!("module 'socket' has no attribute '{name}'"),
            ));
        }
    };
    Ok(ctx.int(i64::from(value)))
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

fn pack_address(ctx: &mut Context, addr: &libc::sockaddr_in) -> Id {
    let (host, port) = native::endpoint(addr);
    let host = ctx.text(host.to_string());
    let port = ctx.int(i64::from(port));
    ctx.alloc(Value::Tuple(vec![host, port]))
}

fn address(ctx: &Context, pair: Id, name: &str) -> Result<libc::sockaddr_in> {
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
    Ok(native::address(
        ctx.str_value(*host)?,
        ctx.as_integer(*port)?,
        name,
    )?)
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
        let addr = pack_address(ctx, &row.addr);
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
        let mode = option(args, 0, kwargs, "mode")?
            .map(|id| ctx.str_value(id))
            .transpose()?
            .unwrap_or("r");
        let buffering = int_option(ctx, args, kwargs, 1, "buffering", -1)?;
        if mode != "rb" || buffering != -1 {
            return Err(Error::new(
                "NotImplementedError",
                "socket.makefile supports the default buffered binary reader",
            ));
        }
        // CPython 3.14 uses io.DEFAULT_BUFFER_SIZE (128 KiB) for socket.makefile.
        let raw = super::ReadSource::Socket(native::SocketInput::new(s));
        let reader = std::io::BufReader::with_capacity(128 * 1024, raw);
        return Ok(
            ctx.alloc(Value::File(std::rc::Rc::new(std::cell::RefCell::new(
                Some(reader),
            )))),
        );
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
        "bind" => native::bind(&s, &address(ctx, argument(args, 0)?, method)?)?,
        "connect" => native::connect_address(&s, &address(ctx, argument(args, 0)?, method)?)?,
        "listen" => native::listen(
            &s,
            int_option(ctx, args, kwargs, 0, "backlog", libc::SOMAXCONN.min(128))?,
        )?,
        "accept" => {
            let (child, peer) = native::accept(&s)?;
            let child = ctx.alloc(Value::Socket(child));
            let peer = pack_address(ctx, &peer);
            return Ok(ctx.alloc(Value::Tuple(vec![child, peer])));
        }
        "getsockname" => return Ok(pack_address(ctx, &native::sockname(&s)?)),
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
