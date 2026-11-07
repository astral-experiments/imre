//! Runtime-loaded Python sockets share descriptor ownership and syscalls with typed sockets.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod message;
use super::{Context, Error, Id, NONE, Result, Value, argument, option};
use crate::objects::socket as native;
use std::time::Duration;

/// Native socket operations published as descriptors on the shared socket layout.
pub(in super::super) const CLASS_METHODS: &[&str] = &[
    "__del__",
    "__init__",
    "_accept",
    "bind",
    "close",
    "connect",
    "connect_ex",
    "detach",
    "fileno",
    "getblocking",
    "getpeername",
    "getsockname",
    "getsockopt",
    "gettimeout",
    "listen",
    "recv",
    "recv_into",
    "recvfrom",
    "recvfrom_into",
    "send",
    "sendall",
    "sendto",
    "setblocking",
    "setsockopt",
    "settimeout",
    "shutdown",
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "sendmsg",
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "recvmsg",
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "recvmsg_into",
];

pub(in super::super) fn install_socket_layout(ctx: &mut Context, class: Id) {
    let mut entries = Vec::new();
    for &name in CLASS_METHODS.iter().chain(["__new__"].iter()) {
        let member = ctx.builtin(&format!("socket.socket.{name}"));
        entries.push((ctx.heap.code_name(name), member));
    }
    for name in ["family", "type", "proto", "timeout"] {
        let get = ctx.builtin(&format!("socket.socket.{name}"));
        let member = ctx.alloc(Value::GetSetDescriptor {
            owner: "socket.socket",
            name,
            get,
            set: None,
        });
        entries.push((ctx.heap.code_name(name), member));
    }
    let dict = ctx.dict(entries);
    ctx.native_type_dicts.insert(class, dict);
}

fn allocate(ctx: &mut Context, class: Id) -> Result<Id> {
    let base = ctx.builtin("socket.socket");
    if class == base {
        return Ok(ctx.alloc(Value::Socket(native::uninitialized()?)));
    }
    if !matches!(ctx.heap.get(class), Value::Class { .. })
        || ctx.allocation_base(class) != Some(base)
    {
        return Err(Error::new(
            "TypeError",
            "_socket.socket.__new__ requires a socket subtype",
        ));
    }
    let handle = native::uninitialized()?;
    let obj = ctx.instance(class, None);
    let Value::Instance { socket, .. } = ctx.heap.get_mut(obj) else {
        unreachable!()
    };
    *socket = Some(handle);
    Ok(obj)
}

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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("AI_CANONNAME", libc::AI_CANONNAME),
    ("AI_NUMERICHOST", libc::AI_NUMERICHOST),
    ("AI_NUMERICSERV", libc::AI_NUMERICSERV),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("AI_V4MAPPED", libc::AI_V4MAPPED),
    ("EAI_SERVICE", libc::EAI_SERVICE),
    ("EAI_NODATA", libc::EAI_NODATA),
    ("EAI_NONAME", libc::EAI_NONAME),
    ("MSG_OOB", libc::MSG_OOB),
    ("MSG_PEEK", libc::MSG_PEEK),
    ("MSG_DONTROUTE", libc::MSG_DONTROUTE),
    ("MSG_DONTWAIT", libc::MSG_DONTWAIT),
    ("MSG_EOR", libc::MSG_EOR),
    ("MSG_TRUNC", libc::MSG_TRUNC),
    ("MSG_CTRUNC", libc::MSG_CTRUNC),
    ("MSG_WAITALL", libc::MSG_WAITALL),
    ("MSG_NOSIGNAL", libc::MSG_NOSIGNAL),
    #[cfg(target_os = "linux")]
    ("MSG_CMSG_CLOEXEC", libc::MSG_CMSG_CLOEXEC),
    #[cfg(target_os = "linux")]
    ("MSG_ERRQUEUE", libc::MSG_ERRQUEUE),
    #[cfg(target_os = "linux")]
    ("MSG_CONFIRM", libc::MSG_CONFIRM),
    #[cfg(target_os = "linux")]
    ("MSG_MORE", libc::MSG_MORE),
    #[cfg(target_os = "linux")]
    ("MSG_FASTOPEN", libc::MSG_FASTOPEN),
    #[cfg(target_os = "macos")]
    ("MSG_EOF", libc::MSG_EOF),
    ("SOL_TCP", libc::IPPROTO_TCP),
    #[cfg(target_os = "linux")]
    ("TCP_QUICKACK", libc::TCP_QUICKACK),
    ("NI_DGRAM", libc::NI_DGRAM),
    ("NI_NAMEREQD", libc::NI_NAMEREQD),
    ("NI_NOFQDN", libc::NI_NOFQDN),
    ("NI_NUMERICHOST", libc::NI_NUMERICHOST),
    ("NI_NUMERICSERV", libc::NI_NUMERICSERV),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("IPV6_RECVHOPLIMIT", libc::IPV6_RECVHOPLIMIT),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("IPV6_HOPLIMIT", libc::IPV6_HOPLIMIT),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("IPV6_RECVTCLASS", libc::IPV6_RECVTCLASS),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("IPV6_TCLASS", libc::IPV6_TCLASS),
    #[cfg(target_os = "linux")]
    ("IPPROTO_UDPLITE", libc::IPPROTO_UDPLITE),
    ("IPPROTO_UDP", libc::IPPROTO_UDP),
    ("SOL_SOCKET", libc::SOL_SOCKET),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ("SCM_RIGHTS", libc::SCM_RIGHTS),
    ("SO_TYPE", libc::SO_TYPE),
    ("SO_ERROR", libc::SO_ERROR),
    ("SO_LINGER", libc::SO_LINGER),
    ("SO_REUSEADDR", libc::SO_REUSEADDR),
    ("SO_REUSEPORT", libc::SO_REUSEPORT),
    ("SOCK_DGRAM", libc::SOCK_DGRAM),
    #[cfg(target_os = "linux")]
    ("SOCK_CLOEXEC", libc::SOCK_CLOEXEC),
    #[cfg(target_os = "linux")]
    ("SOCK_NONBLOCK", libc::SOCK_NONBLOCK),
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

pub(in super::super) const IMPORTED_FUNCTIONS: &[&str] = &[
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "CMSG_LEN",
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "CMSG_SPACE",
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    "gethostbyname_ex",
    "getdefaulttimeout",
    #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
    "getprotobyname",
    "htonl",
    "htons",
    "ntohl",
    "ntohs",
    "setdefaulttimeout",
    "inet_aton",
    "inet_pton",
    "inet_ntoa",
    "inet_ntop",
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
    for &name in IMPORTED_FUNCTIONS {
        let function = ctx.builtin(&format!("_socket.{name}"));
        ctx.bind(scope, name, function);
    }
    let pair = ctx.builtin("_socket.socketpair");
    ctx.bind(scope, "socketpair", pair);
    let resolver = ctx.builtin("_socket.getaddrinfo");
    ctx.bind(scope, "getaddrinfo", resolver);
    let socket_type = attribute(ctx, "socket")?;
    ctx.bind(scope, "SocketType", socket_type);
    let timeout = ctx.builtin("TimeoutError");
    ctx.bind(scope, "timeout", timeout);
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
    for &(name, host) in imre_ast::providers::SOCKET_LOOPBACK_HOSTS {
        let value = ctx.text(host);
        ctx.bind(scope, name, value);
    }
    let getaddrinfo = ctx.builtin("socket.getaddrinfo");
    ctx.bind(scope, "getaddrinfo", getaddrinfo);
    for name in IMPORTED_CONSTANTS
        .iter()
        .map(|&(name, _)| name)
        .chain(RESOLVER_NAMES.iter().copied())
        .chain(["timeout"])
        .chain(IMPORTED_FUNCTIONS.iter().copied())
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
        ("MsgFlag", "MSG_", "enum.IntFlag"),
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
        native::SocketName::Unix(path) => {
            if cfg!(target_os = "linux") && path.first() == Some(&0) {
                return Ok(ctx.alloc(Value::Bytes(path)));
            }
            let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
            Ok(ctx.text_codepoints(codec.decode_filesystem(&path)?))
        }
        native::SocketName::NoAddress => Ok(NONE),
    }
}

fn address(ctx: &mut Context, family: i32, arg: Id, name: &str) -> Result<native::SocketAddress> {
    if family == libc::AF_UNIX {
        let encoded;
        let path = match ctx.heap.get(ctx.scalar_value(arg)) {
            Value::Str(_) | Value::SurrogateStr(_) => {
                encoded = ctx.encode_os_path(arg)?;
                &encoded[..]
            }
            _ => {
                ctx.check_buffer_exporter(arg)?;
                ctx.native_buffer_bytes(arg)?
            }
        };
        return Ok(native::SocketAddress::unix(path)?);
    }
    internet_address(ctx, family, arg, name).map(native::SocketAddress::Internet)
}

fn internet_address(
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
        Value::Str(_) | Value::SurrogateStr(_) => match ctx.heap.get(fields[0]) {
            // CPython's sockaddr converter sends compact ASCII strings directly to libc.
            Value::Str(text) if text.is_ascii() => text.as_bytes().to_vec(),
            _ => idna_host(ctx, fields[0])
                .map_err(|_| Error::new("TypeError", "encoding of hostname failed"))?,
        },
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

/// Encode the original Python string through the same text codec as str.encode.
fn idna_host(ctx: &mut Context, host: Id) -> Result<Vec<u8>> {
    let encoding = ctx.text("idna");
    let encoded = super::codecs::encode_text_object(ctx, host, Some(encoding), None)?;
    Ok(ctx.bytes_like(encoded)?.to_vec())
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
        Value::Str(_) | Value::SurrogateStr(_) => idna_host(ctx, arg)?,
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
    let [host_arg, service_arg, family, ty, protocol, flags] = getaddrinfo_args(ctx, args, kwargs)?;
    let family = ctx.index_c_int(family)?;
    let ty = ctx.index_c_int(ty)?;
    let protocol = ctx.index_c_int(protocol)?;
    let flags = ctx.index_c_int(flags)?;
    let host = match ctx.heap.get(ctx.scalar_value(host_arg)) {
        Value::None => None,
        Value::Bytes(bytes) => Some(bytes.clone()),
        Value::Str(_) | Value::SurrogateStr(_) => Some(idna_host(ctx, host_arg)?),
        _ => {
            return Err(Error::new(
                "TypeError",
                "getaddrinfo() argument 1 must be string or None",
            ));
        }
    };
    let service = if let Some(index) = ctx.index_value(service_arg)? {
        Some(index.to_string().into_bytes())
    } else {
        match ctx.heap.get(ctx.scalar_value(service_arg)) {
            Value::None => None,
            Value::Bytes(bytes) => Some(bytes.clone()),
            Value::Str(s) => Some(s.as_bytes().to_vec()),
            Value::SurrogateStr(_) => {
                Some(ctx.encode_utf8_codepoints(service_arg, false, "strict")?)
            }
            _ => return Err(Error::new("OSError", "Int or String expected")),
        }
    };
    let family_arg = ctx.int(i64::from(family));
    let type_arg = ctx.int(i64::from(ty));
    let protocol_arg = ctx.int(i64::from(protocol));
    super::audit::event(
        ctx,
        "socket.getaddrinfo",
        &[host_arg, service_arg, family_arg, type_arg, protocol_arg],
    )?;
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
fn text_argument(
    ctx: &mut Context,
    name: &str,
    index: Option<usize>,
    value: Id,
) -> Result<std::ffi::CString> {
    let storage = ctx.string_storage(value).ok_or_else(|| {
        let ty = if value == NONE {
            "None"
        } else {
            ctx.type_name(value)
        };
        let ty = &ty[..ty.floor_char_boundary(50)];
        let message = match index {
            Some(index) => format!("{name}() argument {index} must be str, not {ty}"),
            None => format!("{name}() argument must be str, not {ty}"),
        };
        Error::new("TypeError", message)
    })?;
    let text = match ctx.heap.get(storage) {
        Value::Str(text) => std::ffi::CString::new(text.as_bytes()),
        _ => std::ffi::CString::new(ctx.encode_utf8_codepoints(value, false, "strict")?),
    };
    text.map_err(|_| Error::new("ValueError", "embedded null character"))
}

/// Look up a protocol number in the platform's protocol database.
#[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
pub(super) fn getprotobyname(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "getprotobyname() takes no keyword arguments",
        ));
    }
    let [name] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "getprotobyname() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    };
    let name = text_argument(ctx, "getprotobyname", Some(1), *name)?;
    let number = native::protocol_by_name(&name)?;
    Ok(ctx.int(i64::from(number)))
}

/// Convert an unsigned Python index between host and network byte order.
pub(super) fn convert_byte_order(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    use num_traits::{Signed, ToPrimitive};
    let arg = super::super::builtins::one_positional_arg(name, args, kwargs)?;
    let value = ctx.index_integer(arg)?;
    let value = value.big();
    if value.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    // Reversing the host/network conversion uses the same permutation in either direction.
    let (converted, width) = if matches!(name, "_socket.htons" | "_socket.ntohs") {
        (value.to_u16().map(|n| i64::from(n.to_be())), 16)
    } else {
        (value.to_u32().map(|n| i64::from(n.to_be())), 32)
    };
    let converted = converted.ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!("Python int too large for C uint{width}_t"),
        )
    })?;
    Ok(ctx.int(converted))
}

/// Convert the host's IPv4 numbers-and-dots notation to four network-order bytes.
pub(super) fn inet_aton(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let arg = super::super::builtins::one_positional_arg("_socket.inet_aton", args, kwargs)?;
    let text = text_argument(ctx, "inet_aton", None, arg)?;
    unsafe extern "C" {
        #[link_name = "inet_aton"]
        fn c_inet_aton(cp: *const libc::c_char, inp: *mut libc::in_addr) -> libc::c_int;
    }
    let mut addr = libc::in_addr { s_addr: 0 };
    // SAFETY: text is NUL-terminated and addr is a writable IPv4 address.
    if unsafe { c_inet_aton(text.as_ptr(), &mut addr) } == 0 {
        return Err(Error::new(
            "OSError",
            "illegal IP address string passed to inet_aton",
        ));
    }
    Ok(ctx.alloc(Value::Bytes(addr.s_addr.to_ne_bytes().to_vec())))
}

/// Parse a numeric IPv4 or IPv6 address using the platform's inet_pton.
pub(super) fn inet_pton(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    unsafe extern "C" {
        #[link_name = "inet_pton"]
        fn c_inet_pton(
            af: libc::c_int,
            src: *const libc::c_char,
            dst: *mut libc::c_void,
        ) -> libc::c_int;
    }

    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "inet_pton() takes no keyword arguments",
        ));
    }
    let [family, address] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "inet_pton() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    };
    let family = ctx.index_c_int(*family)?;
    let address = text_argument(ctx, "inet_pton", Some(2), *address)?;
    let mut packed = [0u8; 16];
    // SAFETY: address is NUL-terminated and packed fits either supported address family.
    let status = unsafe { c_inet_pton(family, address.as_ptr(), packed.as_mut_ptr().cast()) };
    if status < 0 {
        return Err(super::os_error(std::io::Error::last_os_error()));
    }
    if status == 0 {
        return Err(Error::new(
            "OSError",
            "illegal IP address string passed to inet_pton",
        ));
    }
    let bytes = match family {
        libc::AF_INET => &packed[..4],
        libc::AF_INET6 => &packed[..],
        _ => return Err(Error::new("OSError", "unknown address family")),
    };
    Ok(ctx.alloc(Value::Bytes(bytes.to_vec())))
}

/// Format a contiguous packed address while its Python buffer export is live.
pub(super) fn format_packed_address(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (family, packed) = if name == "_socket.inet_ntoa" {
        let packed = super::super::builtins::one_positional_arg(name, args, kwargs)?;
        (libc::AF_INET, packed)
    } else {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "inet_ntop() takes no keyword arguments",
            ));
        }
        let [family, packed] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "inet_ntop() takes exactly 2 arguments ({} given)",
                    args.len()
                ),
            ));
        };
        (ctx.index_c_int(*family)?, *packed)
    };
    let format = |bytes: &[u8]| -> Result<String> {
        match family {
            libc::AF_INET => {
                let octets = <[u8; 4]>::try_from(bytes).map_err(|_| {
                    if name == "_socket.inet_ntoa" {
                        Error::new("OSError", "packed IP wrong length for inet_ntoa")
                    } else {
                        Error::new("ValueError", "invalid length of packed IP address string")
                    }
                })?;
                Ok(std::net::Ipv4Addr::from(octets).to_string())
            }
            libc::AF_INET6 => {
                let s6_addr = bytes.try_into().map_err(|_| {
                    Error::new("ValueError", "invalid length of packed IP address string")
                })?;
                Ok(native::ipv6_text(&libc::in6_addr { s6_addr })?)
            }
            _ => Err(Error::new(
                "ValueError",
                format!("unknown address family {family}"),
            )),
        }
    };
    let text = if let Some(method) = ctx.python_buffer_method(packed, true)? {
        ctx.with_contiguous_python_buffer(packed, method, 0, |ctx, view| {
            format(ctx.view_bytes(view)?)
        })?
    } else {
        format(ctx.native_buffer_bytes(packed)?)?
    };
    Ok(ctx.text(text))
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
        Query::Name(text_argument(ctx, name, Some(1), args[0])?)
    } else {
        Query::Port(ctx.index_c_int(args[0])?)
    };
    // Parse the optional protocol before checking the converted port's 16-bit range.
    let proto = args
        .get(1)
        .map(|&v| text_argument(ctx, name, Some(2), v))
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn cmsg_size(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arg = super::super::builtins::one_positional_arg(name, args, kwargs)?;
    let size = native::message::control_size(receive_size(ctx, arg)?, name == "CMSG_SPACE")?;
    Ok(ctx.int(size as i64))
}

fn receive_size(ctx: &mut Context, value: Id) -> Result<isize> {
    crate::signed_index(&ctx.index_integer(value)?).ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// Recreate any pointers into the Python heap after a returning signal handler.
fn retry<T>(
    ctx: &mut Context,
    socket: &crate::objects::Object,
    mut attempt: impl FnMut(&mut Context, Option<native::Deadline>) -> Result<T>,
) -> Result<T> {
    let until = native::deadline(socket);
    loop {
        match attempt(ctx, until) {
            Err(err) if ctx.has_errno(&err, libc::EINTR) => {
                ctx.dispatch_signals()?;
                if until.is_some_and(|end| end.remaining().is_zero()) {
                    return Err(Error::new("TimeoutError", "timed out"));
                }
            }
            result => return result,
        }
    }
}

fn receive_into(
    ctx: &mut Context,
    method: &str,
    socket: &crate::objects::Object,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    const FIELDS: [&str; 3] = ["buffer", "nbytes", "flags"];
    let required = super::required_args(method, args, kwargs, &FIELDS)?;
    let buffer = required(0)?;
    let optional = |index: usize| {
        args.get(index).copied().or_else(|| {
            kwargs
                .iter()
                .find(|(key, _)| key == FIELDS[index])
                .map(|(_, value)| *value)
        })
    };
    let (count, peer) = ctx.with_writable_byte_span(
        buffer,
        &format!("{method}() argument 1"),
        |ctx, storage, span| {
            let nbytes = optional(1)
                .map(|value| receive_size(ctx, value))
                .transpose()?
                .unwrap_or(0);
            let flags = optional(2)
                .map(|value| ctx.index_c_int(value))
                .transpose()?
                .unwrap_or(0);
            // PyArg converts its recognized inputs before diagnosing leftover keywords.
            if !kwargs.is_empty() {
                super::super::builtins::bind_builtin_arguments(method, args, kwargs, FIELDS, 1)?;
            }
            let nbytes = usize::try_from(nbytes).map_err(|_| {
                Error::new("ValueError", format!("negative buffersize in {method}"))
            })?;
            let size = if nbytes == 0 { span.len() } else { nbytes };
            if size > span.len() {
                let message = if method == "recv_into" {
                    "buffer too small for requested bytes"
                } else {
                    "nbytes is greater than the length of the buffer"
                };
                return Err(Error::new("ValueError", message));
            }
            retry(ctx, socket, |ctx, until| {
                let target = ctx
                    .writable_storage_bytes(storage)?
                    .get_mut(span.start..span.start + size)
                    .ok_or_else(|| {
                        Error::new("BufferError", "underlying buffer has changed size")
                    })?;
                if method == "recv_into" {
                    Ok((native::recv_into(socket, target, flags, until)?, None))
                } else {
                    let (count, peer) = native::recv_from_into(socket, target, flags, until)?;
                    Ok((count, Some(peer)))
                }
            })
        },
    )?;
    let count = ctx.int(count as i64);
    let Some(peer) = peer else {
        return Ok(count);
    };
    let peer = pack_socket_name(ctx, peer)?;
    Ok(ctx.alloc(Value::Tuple(vec![count, peer])))
}

fn setsockopt(
    ctx: &mut Context,
    socket: &crate::objects::Object,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    use native::SocketOption;

    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "socket.setsockopt() takes no keyword arguments",
        ));
    }
    let wrong_count = || {
        Error::new(
            "TypeError",
            format!(
                "setsockopt() takes exactly 3 arguments ({} given)",
                args.len()
            ),
        )
    };
    if let &[level, name, value, length] = args {
        // A failed four-argument parse falls through to CPython's three-argument buffer form.
        let level = ctx.index_c_int(level).map_err(|_| wrong_count())?;
        let name = ctx.index_c_int(name).map_err(|_| wrong_count())?;
        if value != NONE {
            return Err(wrong_count());
        }
        let length = unsigned_mask(ctx.index_integer(length).map_err(|_| wrong_count())?);
        native::set_option(native::fd(socket), level, name, SocketOption::Null(length))?;
        return Ok(NONE);
    }
    let &[level, name, value] = args else {
        return Err(wrong_count());
    };
    let option =
        |ctx: &mut Context| -> Result<_> { Ok((ctx.index_c_int(level)?, ctx.index_c_int(name)?)) };
    // PyArg tries "iii" before "iiy*". Converting level and name again on the
    // buffer path is observable when either implements __index__.
    let integers = option(ctx).and_then(|(level, name)| Ok((level, name, ctx.index_c_int(value)?)));
    if let Ok((level, name, flag)) = integers {
        native::set_option(native::fd(socket), level, name, SocketOption::Integer(flag))?;
    } else {
        let (level, name) = option(ctx)?;
        ctx.with_read_buffer(value, |ctx, view| {
            let bytes = ctx.native_buffer_bytes(view)?;
            Ok(native::set_option(
                native::fd(socket),
                level,
                name,
                SocketOption::Buffer(bytes),
            )?)
        })?;
    }
    Ok(NONE)
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
    let used = native::get_option_into(native::fd(socket), level, name, &mut bytes)?;
    bytes.truncate(used);
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

fn parse_socket_timeout(ctx: &mut Context, value: Id) -> Result<Option<Duration>> {
    super::select::timeout(ctx, Some(&value), false).map_err(|error| {
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
    })
}

/// Update the default only after the same conversion used by socket.settimeout succeeds.
pub(super) fn setdefaulttimeout(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let value =
        super::super::builtins::one_positional_arg("_socket.setdefaulttimeout", args, kwargs)?;
    let timeout = parse_socket_timeout(ctx, value)?;
    *ctx.socket_default_timeout.lock().unwrap() = timeout;
    Ok(NONE)
}

/// Apply the current default to a new endpoint unless its type requested nonblocking I/O.
fn apply_default_timeout(ctx: &Context, socket: &crate::objects::Object) -> Result<()> {
    if native::get_timeout(socket).is_none()
        && let Some(timeout) = *ctx.socket_default_timeout.lock().unwrap()
        && let Err(error) = native::set_timeout(socket, Some(timeout))
    {
        // The initializer has already adopted the fd. Its receiver may outlive this error.
        native::discard_fd(socket);
        return Err(error.into());
    }
    Ok(())
}

fn new_socket(ctx: &mut Context, socket: crate::ObjectOwner) -> Result<Id> {
    apply_default_timeout(ctx, &socket)?;
    ctx.heap.alloc_descriptor_owner(Value::Socket(socket))
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
    let left = new_socket(ctx, left)?;
    let right = new_socket(ctx, right)?;
    Ok(ctx.alloc(Value::Tuple(vec![left, right])))
}

/// Socket operations shared by raw endpoints and the exclusively owned TLS transport.
pub(super) fn endpoint_method(
    ctx: &mut Context,
    method: &str,
    socket: &crate::objects::Object,
    args: &[Id],
) -> Result<Id> {
    match method {
        "setblocking" => {
            native::set_blocking_mode(socket, ctx.truth(argument(args, 0)?)?)?;
            Ok(NONE)
        }
        "gettimeout" => Ok(native::get_timeout(socket).map_or(NONE, |duration| {
            ctx.alloc(Value::Float(duration.as_secs_f64()))
        })),
        "settimeout" => {
            let value = argument(args, 0)?;
            let timeout = parse_socket_timeout(ctx, value)?;
            native::set_timeout(socket, timeout)?;
            Ok(NONE)
        }
        "getblocking" => Ok(if native::imre_socket_getblocking(socket) {
            super::TRUE
        } else {
            super::FALSE
        }),
        "fileno" => Ok(ctx.int(i64::from(native::fd(socket)))),
        "listen" => {
            let backlog = match args {
                [value] => ctx.index_c_int(*value)?,
                _ => libc::SOMAXCONN.min(128),
            };
            native::listen(socket, backlog)?;
            Ok(NONE)
        }
        "accept" => {
            let (child, peer) = native::accept(socket, || ctx.dispatch_signals())?;
            let peer = pack_socket_name(ctx, peer)?;
            let child = new_socket(ctx, child)?;
            Ok(ctx.alloc(Value::Tuple(vec![child, peer])))
        }
        "getsockname" | "getpeername" => {
            pack_socket_name(ctx, native::socket_name(socket, method == "getpeername")?)
        }
        _ => unreachable!(),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "socket.__new__" => return allocate(ctx, argument(args, 0)?),
        "socket" | "socket.__init__" => {
            let (obj, socket, args) = if name == "socket" {
                let socket = native::uninitialized()?;
                (ctx.alloc(Value::Socket(socket.clone())), socket, args)
            } else {
                let (&obj, args) = args
                    .split_first()
                    .ok_or_else(|| Error::new("TypeError", "missing socket receiver"))?;
                let socket = ctx
                    .heap
                    .get(obj)
                    .socket_handle()
                    .cloned()
                    .ok_or_else(|| Error::new("TypeError", "expected a socket"))?;
                (obj, socket, args)
            };
            let keys = ["family", "type", "proto", "fileno"];
            check_args(name, args, kwargs, keys.len(), &keys)?;
            // Bind like the public Python constructor before invoking any __index__ methods.
            let mut values = [None; 4];
            for (i, key) in keys.iter().enumerate() {
                values[i] = option(args, i, kwargs, key).map_err(|_| {
                    Error::new(
                        "TypeError",
                        format!("socket.__init__() got multiple values for argument '{key}'"),
                    )
                })?;
            }
            let [family, kind, proto, fileno] = values;
            let mut number = |value: Option<Id>| {
                value
                    .map(|v| ctx.index_c_int(v))
                    .transpose()
                    .map(|v| v.unwrap_or(-1))
            };
            let (family, kind, proto) = (number(family)?, number(kind)?, number(proto)?);
            let event = [
                obj,
                ctx.int(i64::from(family)),
                ctx.int(i64::from(kind)),
                ctx.int(i64::from(proto)),
            ];
            super::audit::event(ctx, "socket.__new__", &event)?;
            let raw = match fileno {
                None | Some(NONE) => None,
                Some(value) => Some(ctx.index_c_long(value)? as i32),
            };
            ctx.heap.track_initialized_descriptor(obj)?;
            match raw {
                Some(raw) => native::adopt_into(&socket, raw, family, kind, proto)?,
                None => native::open_into(&socket, family, kind, proto)?,
            }
            apply_default_timeout(ctx, &socket)?;
            return Ok(if name == "socket" { obj } else { NONE });
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
        "gethostbyaddr" | "gethostbyname_ex" => {
            let host = host_argument(ctx, name, args, kwargs)?;
            let entry = if name == "gethostbyaddr" {
                super::audit::event(ctx, "socket.gethostbyaddr", args)?;
                native::hostbyaddr(&host)?
            } else {
                super::audit::event(ctx, "socket.gethostbyname", args)?;
                native::hostbyname_ex(&host)?
            };
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
            let name = ctx.text(native::fqdn_name(host)?);
            let lookup = (|| {
                let host = host_argument(ctx, "gethostbyaddr", &[name], &[])?;
                super::audit::event(ctx, "socket.gethostbyaddr", &[name])?;
                Ok(native::fqdn(&host)?)
            })();
            return match lookup {
                Ok(result) => Ok(ctx.text(result)),
                Err(error) if ctx.is_builtin_error(&error, "OSError") => Ok(name),
                Err(error) => Err(error),
            };
        }
        _ => {}
    }
    let Some(method) = name.strip_prefix("socket.") else {
        return Err(Error::new(
            "TypeError",
            format!("'{name}' is not a socket operation"),
        ));
    };
    let is_message = matches!(method, "sendmsg" | "recvmsg" | "recvmsg_into");
    let descriptor = CLASS_METHODS.contains(&method);
    let (receiver, args) = args.split_first().ok_or_else(|| {
        if descriptor {
            Error::new(
                "TypeError",
                format!("unbound method socket.{method}() needs an argument"),
            )
        } else {
            Error::new("TypeError", "missing socket receiver")
        }
    })?;
    let Some(handle) = ctx.heap.get(*receiver).socket_handle() else {
        if descriptor {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' for '_socket.socket' objects doesn't apply to a '{}' object",
                    ctx.type_name(*receiver)
                ),
            ));
        }
        return Err(Error::new("TypeError", "expected a socket"));
    };
    let s = handle.clone();
    match method {
        "family" => return Ok(ctx.int(i64::from(native::family(&s)))),
        "type" => return Ok(ctx.int(i64::from(native::kind(&s)))),
        "proto" => return Ok(ctx.int(i64::from(native::proto(&s)))),
        "timeout" => {
            return Ok(
                native::get_timeout(&s).map_or(NONE, |t| ctx.alloc(Value::Float(t.as_secs_f64())))
            );
        }
        _ => {}
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if is_message {
        return message::call(ctx, &s, *receiver, method, args, kwargs);
    }
    if method == "_accept" {
        use std::os::fd::{AsRawFd, IntoRawFd};

        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "socket._accept() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("socket._accept() takes no arguments ({} given)", args.len()),
            ));
        }
        let (child, peer) = native::accept_fd(&s, || ctx.dispatch_signals())?;
        let peer = pack_socket_name(ctx, peer)?;
        let fd = ctx.int(i64::from(child.as_raw_fd()));
        let result = ctx.alloc(Value::Tuple(vec![fd, peer]));
        let _ = child.into_raw_fd();
        return Ok(result);
    }
    if matches!(method, "connect" | "connect_ex") {
        let peer = super::super::builtins::one_positional_arg(name, args, kwargs)?;
        let addr = address(ctx, native::family(&s), peer, method)?;
        super::audit::event(ctx, "socket.connect", &[*receiver, peer])?;
        let result = native::connect(&s, &addr, || ctx.dispatch_signals())?;
        if method == "connect" {
            result.map_err(crate::objects::errno_error)?;
            return Ok(NONE);
        }
        let status = match result {
            Ok(()) => 0,
            Err(error) => match error.raw_os_error() {
                Some(errno) => errno,
                None if error.kind() == std::io::ErrorKind::TimedOut => libc::EWOULDBLOCK,
                None => return Err(crate::objects::errno_error(error).into()),
            },
        };
        return Ok(ctx.int(i64::from(status)));
    }
    if method == "getsockopt" {
        return getsockopt(ctx, &s, args, kwargs);
    }
    if method == "setsockopt" {
        return setsockopt(ctx, &s, args, kwargs);
    }
    if method == "sendto" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "sendto() takes no keyword arguments",
            ));
        }
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
    if matches!(method, "recv_into" | "recvfrom_into") {
        return receive_into(ctx, method, &s, args, kwargs);
    }
    let max = match method {
        "bind" | "listen" | "setblocking" | "settimeout" | "shutdown" => 1,
        "recv" | "recvfrom" | "send" | "sendall" => 2,
        _ => 0,
    };
    check_args(method, args, kwargs, max, &[])?;
    match method {
        "__del__" => {
            if native::fd(&s) >= 0 {
                let warning = ctx.checked_format(*receiver, true).and_then(|repr| {
                    ctx.emit_builtin_warning_with_source(
                        crate::objects::exceptions::Kind::ResourceWarning,
                        &format!("unclosed {repr}"),
                        1,
                        *receiver,
                    )
                });
                if let Err(error) = warning {
                    let category = ctx.builtin("Warning");
                    if ctx.raised_error_matches(&error, category) {
                        super::super::unraisable::report(
                            ctx,
                            error,
                            true,
                            Some(*receiver),
                            "Exception ignored while finalizing socket",
                        );
                    }
                }
                // Repr and warning hooks can close, detach or reinitialize this receiver.
                native::discard_fd(&s);
            }
        }
        "close" => native::close(&s)?,
        "detach" => return Ok(ctx.int(i64::from(native::detach(&s)))),
        "fileno" | "listen" | "getsockname" | "getpeername" | "setblocking" | "getblocking"
        | "gettimeout" | "settimeout" => {
            return endpoint_method(ctx, method, &s, args);
        }
        "bind" => native::bind(
            &s,
            &address(ctx, native::family(&s), argument(args, 0)?, method)?,
        )?,
        "shutdown" => native::shutdown(&s, integer(ctx, argument(args, 0)?)?)?,
        "recvfrom" => {
            let n = receive_size(ctx, argument(args, 0)?)?;
            let flags = args
                .get(1)
                .map(|&id| ctx.index_c_int(id))
                .transpose()?
                .unwrap_or(0);
            let size = usize::try_from(n)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recvfrom"))?;
            let (data, peer) = native::recv_from(&s, size, flags, || ctx.dispatch_signals())?;
            let data = ctx.alloc(Value::Bytes(data));
            let peer = pack_socket_name(ctx, peer)?;
            return Ok(ctx.alloc(Value::Tuple(vec![data, peer])));
        }
        "recv" => {
            let n = usize::try_from(ctx.as_integer(argument(args, 0)?)?)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recv"))?;
            let flags = int_option(ctx, args, kwargs, 1, "flags", 0)?;
            let bytes = native::recv_with_signals(&s, n, flags, || ctx.dispatch_signals())?;
            return Ok(ctx.alloc(Value::Bytes(bytes)));
        }
        "send" | "sendall" => {
            let data = argument(args, 0)?;
            ctx.check_buffer_exporter(data)?;
            let bytes = ctx.native_buffer_bytes(data)?;
            let flags = int_option(ctx, args, kwargs, 1, "flags", 0)?;
            if method == "send" {
                return Ok(ctx.int(native::send(&s, bytes, flags)? as i64));
            }
            ctx.with_buffer_export(data, |ctx| send_all_exported(ctx, &s, data, flags))?;
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

/// Reacquire the exported bytes after dispatch, when a signal handler may have mutated them.
pub(super) fn send_all_exported(
    ctx: &mut Context,
    socket: &crate::objects::Object,
    data: Id,
    flags: i32,
) -> Result<()> {
    native::SendAll::run(socket, flags, |send| {
        let done = send.step(ctx.native_buffer_bytes(data)?)?;
        ctx.dispatch_signals()?;
        Ok(done)
    })
}
