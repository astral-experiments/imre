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
    ("IPPROTO_TCP", libc::IPPROTO_TCP),
    ("IPPROTO_IPV6", libc::IPPROTO_IPV6),
    ("IPV6_V6ONLY", libc::IPV6_V6ONLY),
    ("SO_SNDBUF", libc::SO_SNDBUF),
    ("TCP_NODELAY", libc::TCP_NODELAY),
    ("SHUT_RD", libc::SHUT_RD),
    ("SHUT_WR", libc::SHUT_WR),
    ("SHUT_RDWR", libc::SHUT_RDWR),
    ("SOMAXCONN", libc::SOMAXCONN),
];

// socket.py imports these constants once, so deletion must not fall back to a native value.
const IMPORTED_CONSTANTS: &[(&str, i32)] = &[
    ("AI_NUMERICSERV", libc::AI_NUMERICSERV),
    ("EAI_SERVICE", libc::EAI_SERVICE),
    ("EAI_NODATA", libc::EAI_NODATA),
    ("EAI_NONAME", libc::EAI_NONAME),
    ("SOL_TCP", libc::IPPROTO_TCP),
    ("NI_DGRAM", libc::NI_DGRAM),
    ("NI_NAMEREQD", libc::NI_NAMEREQD),
    ("NI_NOFQDN", libc::NI_NOFQDN),
    ("NI_NUMERICHOST", libc::NI_NUMERICHOST),
    ("NI_NUMERICSERV", libc::NI_NUMERICSERV),
    ("IPPROTO_UDP", libc::IPPROTO_UDP),
    ("SOL_SOCKET", libc::SOL_SOCKET),
    ("SO_REUSEADDR", libc::SO_REUSEADDR),
    ("SO_REUSEPORT", libc::SO_REUSEPORT),
    ("SOCK_DGRAM", libc::SOCK_DGRAM),
    ("SOCK_RAW", libc::SOCK_RAW),
    ("SOCK_RDM", libc::SOCK_RDM),
    ("SOCK_SEQPACKET", libc::SOCK_SEQPACKET),
];

const RESOLVER_NAMES: &[&str] = &[
    "gaierror",
    "herror",
    "gethostname",
    "gethostbyname",
    "gethostbyaddr",
    "getnameinfo",
    "getservbyname",
    "getservbyport",
    "if_nameindex",
    "if_nametoindex",
    "if_indextoname",
];

/// Seed the low-level module once so normal module assignment and deletion remain observable.
pub(in super::super) fn install_lowlevel_socket(ctx: &mut Context, scope: Id) -> Result<()> {
    for &(name, value) in CONSTANTS.iter().chain(IMPORTED_CONSTANTS) {
        let value = ctx.int(i64::from(value));
        ctx.bind(scope, name, value);
    }
    for name in RESOLVER_NAMES {
        let value = ctx.builtin(&format!("socket.{name}"));
        ctx.bind(scope, name, value);
    }
    let pair = ctx.builtin("_socket.socketpair");
    ctx.bind(scope, "socketpair", pair);
    let resolver = ctx.builtin("_socket.getaddrinfo");
    ctx.bind(scope, "getaddrinfo", resolver);
    let socket_type = attribute(ctx, "socket")?;
    ctx.bind(scope, "SocketType", socket_type);
    for name in ["has_ipv6", "socket", "close", "error"] {
        let value = attribute(ctx, name)?;
        ctx.bind(scope, name, value);
    }
    Ok(())
}

/// Snapshot the native constants and build the public enum members before socket.py runs.
pub(in super::super) fn import_socket_constants(ctx: &mut Context, scope: Id) -> Result<()> {
    let low = ctx.module("_socket")?;
    ctx.bind(scope, "_socket", low);
    let getaddrinfo = ctx.builtin("socket.getaddrinfo");
    ctx.bind(scope, "getaddrinfo", getaddrinfo);
    for name in IMPORTED_CONSTANTS
        .iter()
        .map(|&(name, _)| name)
        .chain(RESOLVER_NAMES.iter().copied())
    {
        match ctx.lookup_attr(low, name) {
            Ok(value) => ctx.bind(scope, name, value),
            Err(error) if ctx.is_attribute_error(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let Value::Module {
        scope: low_scope, ..
    } = *ctx.heap.get(low)
    else {
        return Err(Error::new("TypeError", "_socket must be a module"));
    };
    let namespace = ctx.module_globals(low_scope)?;
    for (class_name, prefix, base_name) in [
        ("AddressFamily", "AF_", "enum.IntEnum"),
        ("SocketKind", "SOCK_", "enum.IntEnum"),
        ("AddressInfo", "AI_", "enum.IntFlag"),
    ] {
        let enum_scope = ctx.scope("socket", None);
        let module = ctx.text("socket");
        ctx.bind(enum_scope, "__module__", module);
        let mut entries: Vec<_> = ctx
            .dict_entries(namespace)
            .expect("module globals")
            .iter()
            .filter_map(|&(key, value)| {
                let name = ctx.str_value(key).ok()?;
                (name.starts_with(prefix) && !name.bytes().any(|ch| ch.is_ascii_lowercase()))
                    .then(|| (name.to_owned(), value))
            })
            .collect();
        // Enum._convert_ sorts by value, breaking alias ties by name. When
        // replacement exports cannot be ordered together, it sorts by name.
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        if entries.iter().all(|(_, value)| ctx.as_int(*value).is_ok()) {
            entries.sort_by(|(_, a), (_, b)| ctx.as_int(*a).unwrap().cmp(&ctx.as_int(*b).unwrap()));
        }
        for (name, value) in &entries {
            ctx.bind(enum_scope, name, *value);
        }
        let base = ctx.builtin(base_name);
        let class = ctx.allocate_class(class_name.into(), enum_scope, vec![base])?;
        ctx.populate_enum(class, Some(base))?;
        ctx.bind(scope, class_name, class);
        for (name, _) in entries {
            let member = ctx.class_attr(class, &name).expect("declared socket enum");
            ctx.bind(scope, &name, member);
        }
    }
    Ok(())
}

pub(super) fn attribute(ctx: &mut Context, name: &str) -> Result<Id> {
    if !name.starts_with("AF_")
        && !name.starts_with("SOCK_")
        && !name.starts_with("AI_")
        && let Some(&(_, value)) = CONSTANTS.iter().find(|&&(key, _)| key == name)
    {
        return Ok(ctx.int(i64::from(value)));
    }
    match name {
        // IPv6 is compiled in on the supported Linux and macOS targets, independently of
        // whether a particular process can create or bind an IPv6 socket.
        "has_ipv6" => Ok(super::TRUE),
        "socket" => Ok(ctx.builtin("socket.socket")),
        "close" => Ok(ctx.builtin("_socket.close")),
        "getfqdn" => Ok(ctx.alloc(Value::Native("socket.getfqdn".into()))),
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

fn pack_socket_name(ctx: &mut Context, name: native::SocketName) -> Result<Id> {
    match name {
        native::SocketName::Internet(addr) => pack_address(ctx, &addr),
        native::SocketName::UnnamedUnix => Ok(ctx.text("")),
        native::SocketName::NoAddress => Ok(NONE),
    }
}

fn address(
    ctx: &mut Context,
    family: i32,
    pair: Id,
    name: &str,
) -> Result<native::InternetAddress> {
    let ipv6 = family == libc::AF_INET6;
    let family_name = if ipv6 { "AF_INET6" } else { "AF_INET" };
    let fields = ctx.tuple_items(pair)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "{name}(): {family_name} address must be tuple, not {}",
                ctx.type_name(pair)
            ),
        )
    })?;
    if ipv6 && !(2..=4).contains(&fields.len()) {
        return Err(Error::new(
            "TypeError",
            "AF_INET6 address must be a tuple (host, port[, flowinfo[, scopeid]])",
        ));
    }
    if !ipv6 && fields.len() != 2 {
        return Err(Error::new(
            "TypeError",
            "AF_INET address must be a pair (host, port)",
        ));
    }
    // Convert in CPython's argument order; __index__ may run arbitrary code.
    let host = match ctx.heap.get(ctx.scalar_value(fields[0])) {
        Value::Bytes(_) | Value::ByteArray(_) => ctx.bytes_like(fields[0])?.to_vec(),
        Value::Str(_) | Value::SurrogateStr(_) => {
            let text = ctx.str_value(fields[0])?;
            if ipv6 && !text.is_ascii() {
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
        .ok_or_else(|| Error::new("OverflowError", format!("{name}(): port must be 0-65535.")))?;
    if !ipv6 {
        return Ok(native::InternetAddress::V4(native::address(
            &host, port, name,
        )?));
    }
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
    Ok(native::ipv6_address(&host, port, flow, scope, name)?)
}

fn unsigned_mask(value: crate::Int) -> u32 {
    use num_traits::ToPrimitive;
    (value.big().as_ref() & num_bigint::BigInt::from(u32::MAX))
        .to_u32()
        .expect("masked uint32")
}

/// PyArg's "et" conversion IDNA-encodes str and treats bytes/bytearray as already encoded.
fn host_argument(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Vec<u8>> {
    let arg = super::super::builtins::one_positional_arg(name, args, kwargs)?;
    let bytes = match ctx.heap.get(ctx.scalar_value(arg)) {
        Value::Bytes(_) | Value::ByteArray(_) => ctx.bytes_like(arg)?.to_vec(),
        Value::Str(_) | Value::SurrogateStr(_) => {
            let host = ctx.str_value(arg)?;
            if !host.is_ascii() {
                return Err(Error::new(
                    "NotImplementedError",
                    "Unicode host names require IDNA encoding",
                ));
            }
            // Match the ASCII fast path in encodings.idna; the final label may be empty.
            let empty = if host.starts_with('.') {
                Some(0)
            } else {
                host.find("..").map(|i| i + 1)
            };
            if let Some(i) = empty {
                return Err(ctx.string_encoding_error(
                    arg,
                    "idna",
                    "label empty",
                    u32::from(b'.'),
                    i..i + 1,
                ));
            }
            let mut offset = 0;
            for label in host.split('.') {
                if label.len() >= 64 {
                    return Err(ctx.string_encoding_error(
                        arg,
                        "idna",
                        "label too long",
                        0,
                        offset..offset + label.len(),
                    ));
                }
                offset += label.len() + 1;
            }
            host.as_bytes().to_vec()
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{name}() argument 1 must be str, bytes or bytearray, not {}",
                    ctx.type_name(arg)
                ),
            ));
        }
    };
    if bytes.contains(&0) {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() argument 1 must be encoded string without null bytes, not {}",
                ctx.type_name(arg)
            ),
        ));
    }
    Ok(bytes)
}

fn interface_name(ctx: &mut Context, name: &std::ffi::CStr) -> Result<Id> {
    let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
    Ok(ctx.text_codepoints(codec.decode_filesystem(name.to_bytes())?))
}

fn if_nameindex(ctx: &mut Context) -> Result<Id> {
    struct Interfaces(*mut libc::if_nameindex);

    impl Drop for Interfaces {
        fn drop(&mut self) {
            // SAFETY: this guard owns the array returned by if_nameindex.
            unsafe { libc::if_freenameindex(self.0) };
        }
    }

    // SAFETY: if_nameindex has no arguments and returns a new sentinel-terminated array.
    let pointer = unsafe { libc::if_nameindex() };
    if pointer.is_null() {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    let interfaces = Interfaces(pointer);
    let mut entry = interfaces.0;
    let mut result = Vec::new();
    // SAFETY: the array and each terminated name are valid until the guard drops.
    unsafe {
        while (*entry).if_index != 0 {
            let index = ctx.int(i64::from((*entry).if_index));
            let name = interface_name(ctx, std::ffi::CStr::from_ptr((*entry).if_name))?;
            result.push(ctx.alloc(Value::Tuple(vec![index, name])));
            entry = entry.add(1);
        }
    }
    Ok(ctx.alloc(Value::List(result)))
}

// Some libc implementations leave errno unchanged when an interface is missing.
fn set_interface_errno(errno: libc::c_int) {
    // SAFETY: each accessor returns this thread's writable errno.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = errno;
        }
        #[cfg(target_vendor = "apple")]
        {
            *libc::__error() = errno;
        }
        #[cfg(target_os = "android")]
        {
            *libc::__errno() = errno;
        }
    }
}

fn if_nametoindex(ctx: &mut Context, arg: Id) -> Result<Id> {
    let path = super::posixpath::fspath(ctx, arg)?;
    let name = super::os::c_path(ctx, path, "embedded null byte")?;
    set_interface_errno(libc::ENODEV);
    // SAFETY: name is a live, NUL-terminated filesystem spelling.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(ctx.int(i64::from(index)))
}

fn if_indextoname(ctx: &mut Context, arg: Id) -> Result<Id> {
    use num_traits::{Signed, ToPrimitive};
    let value = ctx.index_integer(arg)?;
    let value = value.big();
    if value.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    let index = value
        .to_u32()
        .ok_or_else(|| Error::new("OverflowError", "Python int too large for C unsigned int"))?;
    let mut name = [0; libc::IF_NAMESIZE + 1];
    set_interface_errno(libc::ENXIO);
    // SAFETY: the buffer holds at least IF_NAMESIZE bytes; on success libc terminates it.
    let ptr = unsafe { libc::if_indextoname(index, name.as_mut_ptr()) };
    if ptr.is_null() {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    // SAFETY: a successful call returns a pointer into the live name buffer.
    interface_name(ctx, unsafe { std::ffi::CStr::from_ptr(ptr) })
}

fn getaddrinfo_args(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<[Id; 6]> {
    let values = super::super::builtins::bind_builtin_arguments(
        "getaddrinfo",
        args,
        kwargs,
        ["host", "port", "family", "type", "proto", "flags"],
        2,
    )?;
    let zero = ctx.int(0);
    Ok(values.map(|value| value.unwrap_or(zero)))
}

/// An importlib-only executable may use socket without its bundled Python source.
fn public_getaddrinfo(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let args = getaddrinfo_args(ctx, args, kwargs)?;
    let socket = ctx.module("socket")?;
    let low = ctx.lookup_attr(socket, "_socket")?;
    let resolver = ctx.lookup_attr(low, "getaddrinfo")?;
    let rows = ctx.call(resolver, args.to_vec(), Vec::new())?;
    let iter = ctx.iterator(rows)?;
    let mut result = Vec::new();
    while let Some(row) = ctx.next_item(iter)? {
        let mut fields = ctx.unpack_items(row, 5, None)?;
        for (value, name) in fields.iter_mut().zip(["AddressFamily", "SocketKind"]) {
            let class = ctx.lookup_attr(socket, name)?;
            match ctx.call(class, vec![*value], Vec::new()) {
                Ok(member) => *value = member,
                Err(error) if ctx.is_builtin_error(&error, "ValueError") => {}
                Err(error) => return Err(error),
            }
        }
        result.push(ctx.alloc(Value::Tuple(fields)));
    }
    Ok(ctx.alloc(Value::List(result)))
}

pub(super) fn getaddrinfo(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [host, service, family, ty, protocol, flags] = getaddrinfo_args(ctx, args, kwargs)?;
    let family = ctx.index_c_int(family)?;
    let ty = ctx.index_c_int(ty)?;
    let protocol = ctx.index_c_int(protocol)?;
    let flags = ctx.index_c_int(flags)?;
    let host = match ctx.heap.get(ctx.scalar_value(host)) {
        Value::None => None,
        Value::Bytes(bytes) => Some(bytes.clone()),
        _ => Some(ctx.str_value(host)?.as_bytes().to_vec()),
    };
    let service = if let Some(index) = ctx.index_value(service)? {
        Some(index.to_string().into_bytes())
    } else {
        match ctx.heap.get(ctx.scalar_value(service)) {
            Value::None => None,
            Value::Bytes(bytes) => Some(bytes.clone()),
            Value::Str(s) => Some(s.as_bytes().to_vec()),
            Value::SurrogateStr(_) => Some(ctx.encode_utf8_codepoints(service, false, "strict")?),
            _ => return Err(Error::new("OSError", "Int or String expected")),
        }
    };
    // CPython passes the NUL-terminated host and service buffers to libc.
    fn c_string_prefix(value: &[u8]) -> &[u8] {
        value.split(|&byte| byte == 0).next().unwrap_or_default()
    }
    let rows = native::getaddrinfo(
        host.as_deref().map(c_string_prefix),
        service.as_deref().map(c_string_prefix),
        family,
        ty,
        protocol,
        flags,
    )?;
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let family = ctx.int(i64::from(row.addr.family()));
        let ty = ctx.int(i64::from(row.ty));
        let protocol = ctx.int(i64::from(row.protocol));
        let name = ctx.text(row.canonical);
        let addr = pack_address(ctx, &row.addr)?;
        result.push(ctx.alloc(Value::Tuple(vec![family, ty, protocol, name, addr])));
    }
    Ok(ctx.alloc(Value::List(result)))
}

/// Parse a PyArg "s" argument as strict UTF-8 without calling str subclass methods.
fn service_text(
    ctx: &mut Context,
    name: &str,
    index: usize,
    value: Id,
) -> Result<std::ffi::CString> {
    let storage = ctx.string_storage(value).ok_or_else(|| {
        let ty = if value == NONE {
            "None"
        } else {
            ctx.type_name(value)
        };
        Error::new(
            "TypeError",
            format!("{name}() argument {index} must be str, not {ty}"),
        )
    })?;
    let text = match ctx.heap.get(storage) {
        Value::Str(text) => std::ffi::CString::new(text.as_bytes()),
        _ => std::ffi::CString::new(ctx.encode_utf8_codepoints(value, false, "strict")?),
    };
    text.map_err(|_| Error::new("ValueError", "embedded null character"))
}

fn lookup_service(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if !(1..=2).contains(&args.len()) {
        let bound = if args.is_empty() {
            "at least 1 argument"
        } else {
            "at most 2 arguments"
        };
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes {bound} ({} given)", args.len()),
        ));
    }
    enum Query {
        Name(std::ffi::CString),
        Port(i32),
    }
    let query = if name == "getservbyname" {
        Query::Name(service_text(ctx, name, 1, args[0])?)
    } else {
        Query::Port(ctx.index_c_int(args[0])?)
    };
    // Parse the optional protocol before checking the converted port's 16-bit range.
    let proto = args
        .get(1)
        .map(|&v| service_text(ctx, name, 2, v))
        .transpose()?;
    let audit_proto = proto
        .as_ref()
        .map_or(NONE, |s| ctx.text(s.to_str().expect("parsed UTF-8")));
    match query {
        Query::Name(service) => {
            let audit_name = ctx.text(service.to_str().expect("parsed UTF-8"));
            super::audit::event(ctx, "socket.getservbyname", &[audit_name, audit_proto])?;
            Ok(ctx.int(i64::from(native::service_by_name(
                &service,
                proto.as_deref(),
            )?)))
        }
        Query::Port(port) => {
            let port = u16::try_from(port)
                .map_err(|_| Error::new("OverflowError", "getservbyport: port must be 0-65535."))?;
            let audit_port = ctx.int(i64::from(port));
            super::audit::event(ctx, "socket.getservbyport", &[audit_port, audit_proto])?;
            Ok(ctx.text(native::service_by_port(port, proto.as_deref())?))
        }
    }
}

fn getnameinfo(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "getnameinfo() takes no keyword arguments",
        ));
    }
    let &[sockaddr, flags] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "getnameinfo() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    };
    let flags = ctx.index_c_int(flags)?;
    let fields = ctx
        .tuple_items(sockaddr)?
        .ok_or_else(|| Error::new("TypeError", "getnameinfo() argument 1 must be a tuple"))?;
    if !(2..=4).contains(&fields.len()) {
        return Err(Error::new(
            "TypeError",
            "getnameinfo(): illegal sockaddr argument",
        ));
    }
    let host = ctx
        .string_storage(fields[0])
        .ok_or_else(|| Error::new("TypeError", "getnameinfo(): illegal sockaddr argument"))?;
    let host = ctx.encode_utf8_codepoints(host, false, "strict")?;
    if host.contains(&0) {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    let port = ctx.index_c_int(fields[1])?;
    let mut unsigned = |&id| ctx.index_integer(id).map(unsigned_mask);
    let flow = fields.get(2).map(&mut unsigned).transpose()?;
    let scope = fields.get(3).map(unsigned).transpose()?.unwrap_or(0);
    if flow.is_some_and(|v| v > 0xfffff) {
        return Err(Error::new(
            "OverflowError",
            "getnameinfo(): flowinfo must be 0-1048575.",
        ));
    }
    super::audit::event(ctx, "socket.getnameinfo", &[sockaddr])?;
    let (host, service) = native::getnameinfo(&host, port, flow, scope, flags)?;
    let host = ctx.text(host);
    let service = ctx.text(service);
    Ok(ctx.alloc(Value::Tuple(vec![host, service])))
}

fn getsockopt(
    ctx: &mut Context,
    socket: &crate::objects::Object,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "socket.getsockopt() takes no keyword arguments",
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
            format!(
                "getsockopt() takes {bound} arguments ({} given)",
                args.len()
            ),
        ));
    }
    let level = ctx.index_c_int(args[0])?;
    let name = ctx.index_c_int(args[1])?;
    let len = args
        .get(2)
        .map(|&id| ctx.index_c_int(id))
        .transpose()?
        .unwrap_or(0);
    if len == 0 {
        return Ok(ctx.int(i64::from(native::get_option(socket, level, name)?)));
    }
    if !(1..=1024).contains(&len) {
        return Err(Error::new("OSError", "getsockopt buflen out of range"));
    }
    let mut bytes = vec![0; len as usize];
    let used = native::get_option_into(socket, level, name, &mut bytes)?;
    bytes.truncate(used);
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

pub(super) fn close(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let arg = super::super::builtins::one_positional_arg("_socket.close", args, kwargs)?;
    // CPython converts through C long before casting to its POSIX socket descriptor.
    let fd = ctx.index_c_long(arg)? as i32;
    native::close_fd(fd).map_err(crate::objects::errno_error)?;
    Ok(NONE)
}

pub(super) fn socketpair(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    check_args("socketpair", args, kwargs, 3, &[])?;
    let mut values = [libc::AF_UNIX, libc::SOCK_STREAM, 0];
    for (value, &arg) in values.iter_mut().zip(args) {
        *value = ctx.index_c_int(arg)?;
    }
    let [family, kind, proto] = values;
    let (left, right) = native::socketpair(family, kind, proto)?;
    let left = ctx.alloc(Value::Socket(left));
    let right = ctx.alloc(Value::Socket(right));
    Ok(ctx.alloc(Value::Tuple(vec![left, right])))
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
        "if_nameindex" => {
            check_args(name, args, kwargs, 0, &[])?;
            return if_nameindex(ctx);
        }
        "if_nametoindex" => {
            let arg = super::super::builtins::one_positional_arg(name, args, kwargs)?;
            return if_nametoindex(ctx, arg);
        }
        "if_indextoname" => {
            let arg = super::super::builtins::one_positional_arg(name, args, kwargs)?;
            return if_indextoname(ctx, arg);
        }
        "getaddrinfo" => return public_getaddrinfo(ctx, args, kwargs),
        "getnameinfo" => return getnameinfo(ctx, args, kwargs),
        "getservbyname" | "getservbyport" => return lookup_service(ctx, name, args, kwargs),
        "gethostname" => {
            check_args(name, args, kwargs, 0, &[])?;
            super::audit::event(ctx, "socket.gethostname", args)?;
            let raw = native::hostname()?;
            let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
            return Ok(ctx.text_codepoints(codec.decode_filesystem(&raw)?));
        }
        "gethostbyname" => {
            let host = host_argument(ctx, name, args, kwargs)?;
            super::audit::event(ctx, "socket.gethostbyname", args)?;
            return Ok(ctx.text(native::hostbyname(&host)?.to_string()));
        }
        "gethostbyaddr" => {
            let host = host_argument(ctx, name, args, kwargs)?;
            super::audit::event(ctx, "socket.gethostbyaddr", args)?;
            let entry = native::hostbyaddr(&host)?;
            let name = ctx.text(entry.name);
            let aliases = entry.aliases.into_iter().map(|s| ctx.text(s)).collect();
            let aliases = ctx.alloc(Value::List(aliases));
            let addresses = entry.addresses.into_iter().map(|s| ctx.text(s)).collect();
            let addresses = ctx.alloc(Value::List(addresses));
            return Ok(ctx.alloc(Value::Tuple(vec![name, aliases, addresses])));
        }
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
    if method == "getsockopt" {
        return getsockopt(ctx, &s, args, kwargs);
    }
    if method == "makefile" {
        check_args(
            method,
            args,
            kwargs,
            2,
            &["mode", "buffering", "encoding", "errors", "newline"],
        )?;
        let mode = option(args, 0, kwargs, "mode")?.unwrap_or_else(|| ctx.text("r"));
        let storage = ctx
            .string_storage(mode)
            .ok_or_else(|| Error::new("TypeError", "expected str"))?;
        let text = match ctx.heap.get(storage) {
            Value::Str(text) if text.bytes().all(|c| matches!(c, b'r' | b'w' | b'b')) => text,
            _ => {
                return Err(Error::new(
                    "ValueError",
                    format!("invalid mode {} (only r, w, b allowed)", ctx.repr(mode)),
                ));
            }
        };
        let binary = text.contains('b');
        let write = text.contains('w');
        let buffering = match option(args, 1, kwargs, "buffering")? {
            None | Some(NONE) => -1,
            Some(value) => integer(ctx, value)?,
        };
        if !binary && buffering == 0 {
            return Err(Error::new(
                "ValueError",
                "unbuffered streams must be binary",
            ));
        }
        if write || buffering >= 0 {
            return Err(Error::new(
                "NotImplementedError",
                "socket.makefile supports default buffered readers",
            ));
        }
        let wrapper = if binary {
            None
        } else {
            let io = ctx.module("io")?;
            Some(ctx.lookup_attr(io, "TextIOWrapper")?)
        };
        let roots = ctx.heap.temporary_mark();
        let reader = super::ReadSource::Socket(native::SocketInput::try_new(s)?);
        let buffer = ctx.alloc(super::BinaryFile::reader(reader, None).into_value()?);
        let Some(cls) = wrapper else {
            return Ok(buffer);
        };
        let encoding = option(&[], 0, kwargs, "encoding")?.unwrap_or(NONE);
        let errors = option(&[], 0, kwargs, "errors")?.unwrap_or(NONE);
        let newline = option(&[], 0, kwargs, "newline")?.unwrap_or(NONE);
        let args = vec![buffer, encoding, errors, newline];
        return ctx
            .call(cls, args, Vec::new())
            .and_then(|text| {
                ctx.set_attr(text, "mode", mode)?;
                Ok(text)
            })
            .inspect_err(|error| {
                // A replacement text factory can retain the socket reader before raising.
                ctx.heap.release_temporaries_since(roots);
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
                ctx.collect_unreachable();
            });
    }
    let max = match method {
        "bind" | "connect" | "listen" | "setblocking" | "settimeout" | "shutdown" => 1,
        "recv" | "recvfrom" | "send" | "sendall" => 2,
        "setsockopt" | "__exit__" | "recv_into" | "sendto" => 3,
        _ => 0,
    };
    check_args(method, args, kwargs, max, &[])?;
    match method {
        "__enter__" => return Ok(*receiver),
        "__exit__" | "close" => native::close(&s)?,
        "fileno" => return Ok(ctx.int(i64::from(native::fd(&s)))),
        "get_inheritable" => {
            let enabled = crate::objects::os::INHERITABLE.enabled(native::fd(&s))?;
            return Ok(if enabled { super::TRUE } else { super::FALSE });
        }
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
            let peer = pack_address(ctx, &peer)?;
            let child = ctx.alloc(Value::Socket(child));
            return Ok(ctx.alloc(Value::Tuple(vec![child, peer])));
        }
        "getsockname" | "getpeername" => {
            return pack_socket_name(ctx, native::socket_name(&s, method == "getpeername")?);
        }
        "setblocking" => native::set_blocking_mode(&s, ctx.as_integer(argument(args, 0)?)? != 0)?,
        "gettimeout" => {
            return Ok(native::get_timeout(&s).map_or(NONE, |duration| {
                ctx.alloc(Value::Float(duration.as_secs_f64()))
            }));
        }
        "settimeout" => {
            let value = argument(args, 0)?;
            let timeout = super::select::timeout(ctx, Some(&value), false).map_err(|error| {
                if error.kind == "ValueError" && error.message == "timeout must be non-negative" {
                    Error::new("ValueError", "Timeout value out of range")
                } else if error.kind == "TypeError" {
                    Error::new(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer or float",
                            ctx.type_name(value)
                        ),
                    )
                } else {
                    error
                }
            })?;
            native::set_timeout(&s, timeout)?;
        }
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
        "recvfrom" => {
            let n =
                crate::signed_index(&ctx.index_integer(argument(args, 0)?)?).ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?;
            let flags = args
                .get(1)
                .map(|&id| ctx.index_c_int(id))
                .transpose()?
                .unwrap_or(0);
            let size = usize::try_from(n)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recvfrom"))?;
            let (data, peer) = native::recv_from(&s, size, flags)?;
            let data = ctx.alloc(Value::Bytes(data));
            let peer = pack_socket_name(ctx, peer)?;
            return Ok(ctx.alloc(Value::Tuple(vec![data, peer])));
        }
        "sendto" => {
            let (data, flags, peer) = match *args {
                [data, peer] => (data, None, peer),
                [data, flags, peer] => (data, Some(flags), peer),
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("sendto() takes 2 or 3 arguments ({} given)", args.len()),
                    ));
                }
            };
            ctx.check_buffer_exporter(data)?;
            ctx.native_buffer_bytes(data)?;
            let count = ctx.with_buffer_export(data, |ctx| {
                let flags = flags
                    .map(|id| ctx.index_c_int(id))
                    .transpose()?
                    .unwrap_or(0);
                let addr = address(ctx, native::family(&s), peer, method)?;
                super::audit::event(ctx, "socket.sendto", &[*receiver, peer])?;
                Ok(native::send_to(
                    &s,
                    ctx.native_buffer_bytes(data)?,
                    flags,
                    &addr,
                )?)
            })?;
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
                return Ok(ctx.int(native::send(&s, bytes, flags)? as i64));
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
