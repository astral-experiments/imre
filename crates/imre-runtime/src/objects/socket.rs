//! One socket object owns a non-inheritable fd. All aliases observe explicit
//! close; dropping the last owner closes silently, including on error paths.
use super::exceptions::{Error, Kind, Result, finish};
use super::records::{RecordDescriptor, imre_record_new};
use super::{Bytes, Int, Object, Storage, StorageCell, Str, c_int, errno_error};
use num_traits::ToPrimitive;
use std::ffi::{CStr, CString};
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::ptr;

pub(super) struct Socket {
    fd: RawFd,
    blocking: bool,
}

impl Socket {
    fn close(&mut self) -> io::Result<()> {
        let fd = std::mem::replace(&mut self.fd, -1);
        if fd == -1 {
            return Ok(());
        }
        // SAFETY: the object relinquishes the descriptor before close. Do not retry
        // on EINTR: the kernel may have reused the number. Python can also have
        // explicitly closed fileno() via os.close; EBADF is an ordinary socket error.
        match checked(unsafe { libc::close(fd) }) {
            Ok(_) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ECONNRESET) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn state<T>(s: &Object, f: impl FnOnce(&mut Socket) -> T) -> T {
    // SAFETY: the compiler roots an exact socket; no syscall or closure calls Python.
    unsafe {
        s.0.with_mut(|v| match v {
            Storage::Socket(s) => f(s),
            _ => unreachable!(),
        })
    }
}

fn fd(s: &Object) -> RawFd {
    state(s, |s| s.fd)
}

fn checked(ret: i32) -> io::Result<i32> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

fn retry<T>(mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match call() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn word(n: i32) -> usize {
    Int::small(i64::from(n)).unwrap().into_raw()
}

macro_rules! socket_constants {
    ($($name:ident = $value:expr),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub static $name: usize = (($value as usize) << 1) | 1;
    )*};
}

socket_constants! {
    imre_socket_af_inet = libc::AF_INET,
    imre_socket_sock_stream = libc::SOCK_STREAM,
    imre_socket_sol_socket = libc::SOL_SOCKET,
    imre_socket_so_reuseaddr = libc::SO_REUSEADDR,
    imre_socket_ipproto_tcp = libc::IPPROTO_TCP,
    imre_socket_tcp_nodelay = libc::TCP_NODELAY,
    imre_socket_somaxconn = libc::SOMAXCONN,
    imre_socket_so_sndbuf = libc::SO_SNDBUF,
    imre_socket_so_rcvbuf = libc::SO_RCVBUF,
    imre_socket_so_linger = libc::SO_LINGER,
    imre_socket_msg_peek = libc::MSG_PEEK,
    imre_socket_shut_rd = libc::SHUT_RD,
    imre_socket_shut_wr = libc::SHUT_WR,
    imre_socket_shut_rdwr = libc::SHUT_RDWR,

    imre_socket_default_backlog = if libc::SOMAXCONN < 128 { libc::SOMAXCONN } else { 128 },
}

fn setopt(fd: RawFd, level: i32, name: i32, value: i32) -> io::Result<()> {
    // SAFETY: the kernel reads exactly one initialized c_int from value.
    checked(unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&raw const value).cast(),
            size_of_val(&value) as _,
        )
    })?;
    Ok(())
}

fn handle(raw: RawFd, blocking: bool) -> io::Result<crate::ObjectOwner> {
    // SAFETY: the kernel transferred a new fd; every path now has an owner.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: the fresh descriptor is exclusively owned until setup completes.
        checked(unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) })?;
    }
    #[cfg(target_vendor = "apple")]
    setopt(raw, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1)?;
    Ok(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Socket(Socket {
            fd: fd.into_raw_fd(),
            blocking,
        }),
    ))))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_new(family: usize, ty: usize, proto: usize) -> *const Object {
    finish(
        (|| {
            // As in socket.socket.__init__, -1 requests the default for this field.
            let family = match c_int(family)? {
                -1 => libc::AF_INET,
                n => n,
            };
            let ty = match c_int(ty)? {
                -1 => libc::SOCK_STREAM,
                n => n,
            };
            let proto = match c_int(proto)? {
                -1 => 0,
                n => n,
            };
            if family != libc::AF_INET || ty != libc::SOCK_STREAM {
                return Err(Error::new(
                    Kind::NotImplementedError,
                    "only AF_INET stream sockets are supported",
                ));
            }
            #[cfg(target_os = "linux")]
            let ty = ty | libc::SOCK_CLOEXEC;
            // SAFETY: socket consumes only integer arguments; handle acquires a successful fd.
            let raw = checked(unsafe { libc::socket(family, ty, proto) }).map_err(errno_error)?;
            handle(raw, true)
                .map(crate::ObjectOwner::into_raw)
                .map_err(errno_error)
        })(),
        ptr::null(),
    )
}

fn resolve(host: &str) -> Result<Ipv4Addr> {
    match host {
        "" => return Ok(Ipv4Addr::UNSPECIFIED),
        "<broadcast>" => return Ok(Ipv4Addr::BROADCAST),
        _ => {}
    }
    if let Ok(ip) = host.parse() {
        return Ok(ip);
    }
    let host = CString::new(host)
        .map_err(|_| Error::new(Kind::TypeError, "host name must not contain null character"))?;
    // SAFETY: zeros initialize the optional pointers and flags in the hints.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    let mut res = ptr::null_mut();
    // SAFETY: host is terminated, hints is initialized, and res is an out pointer.
    let code =
        unsafe { libc::getaddrinfo(host.as_ptr(), ptr::null(), &raw const hints, &raw mut res) };
    if code == libc::EAI_SYSTEM {
        return Err(errno_error(io::Error::last_os_error()));
    }
    if code != 0 {
        // SAFETY: gai_strerror returns a static terminated message for a getaddrinfo result.
        let message = unsafe { CStr::from_ptr(libc::gai_strerror(code)) }.to_string_lossy();
        return Err(Error::from_errno(
            Kind::GaiError,
            code,
            &message,
            None,
            None,
        ));
    }
    // SAFETY: AF_INET hints guarantee an IPv4 sockaddr in the first result.
    // Take its address by value before freeing the entire result list.
    let octets = unsafe {
        let ip = (*(*res).ai_addr.cast::<libc::sockaddr_in>())
            .sin_addr
            .s_addr;
        libc::freeaddrinfo(res);
        ip.to_ne_bytes()
    };
    Ok(Ipv4Addr::from(octets))
}

/// Reverse resolution must consider aliases before falling back to the primary name.
/// Copy both while the legacy resolver's hostent is protected.
fn fqdn(name: &str) -> Result<String> {
    use std::net::{IpAddr, ToSocketAddrs};
    use std::sync::Mutex;
    unsafe extern "C" {
        fn gethostbyaddr(addr: *const libc::c_void, len: libc::socklen_t, family: libc::c_int) -> *mut libc::hostent;
    }
    static REVERSE: Mutex<()> = Mutex::new(());
    let mut name = name.trim().to_owned();
    if name.is_empty() || name == "0.0.0.0" {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is writable; a final zero bounds CStr even on truncation.
        let n = buffer.len() - 1;
        checked(unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), n) }).map_err(errno_error)?;
        name = CStr::from_bytes_until_nul(&buffer).unwrap().to_string_lossy().into_owned();
    }
    if name.as_bytes().contains(&0) {
        return Err(Error::new(Kind::TypeError, "gethostbyaddr() argument 1 must be encoded string without null bytes"));
    }
    let addr = name.parse::<IpAddr>().ok().or_else(|| {
        (name.as_str(), 0).to_socket_addrs().ok()?.next().map(|address| address.ip())
    });
    let Some(addr) = addr else { return Ok(name); };
    let _guard = REVERSE.lock().unwrap_or_else(|error| error.into_inner());
    // SAFETY: the address bytes live through gethostbyaddr. Its terminated name
    // and aliases remain valid until another legacy lookup on this thread.
    unsafe {
        let host = match addr {
            IpAddr::V4(ip) => gethostbyaddr(ip.octets().as_ptr().cast(), 4, libc::AF_INET),
            IpAddr::V6(ip) => gethostbyaddr(ip.octets().as_ptr().cast(), 16, libc::AF_INET6),
        };
        if host.is_null() || (*host).h_name.is_null() { return Ok(name); }
        let primary = CStr::from_ptr((*host).h_name).to_string_lossy().into_owned();
        if primary.contains('.') { return Ok(primary); }
        let mut aliases = (*host).h_aliases;
        while !aliases.is_null() && !(*aliases).is_null() {
            let alias = CStr::from_ptr(*aliases).to_string_lossy();
            if alias.contains('.') { return Ok(alias.into_owned()); }
            aliases = aliases.add(1);
        }
        Ok(primary)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getfqdn(name: &Str) -> *const Str {
    finish(fqdn(name).map(|name| crate::StrOwner::into_raw(crate::StrOwner::new(Str::from(name)))), ptr::null())
}

fn address(host: &str, port: usize, name: &str) -> Result<libc::sockaddr_in> {
    let port_error = || {
        Error::new(
            Kind::OverflowError,
            format!("{name}(): port must be 0-65535."),
        )
    };
    let port = c_int(port).map_err(|_| port_error())?;
    let ip = resolve(host)?;
    let port = u16::try_from(port).map_err(|_| port_error())?;
    // SAFETY: zero is a valid initial sockaddr_in, including target-specific padding.
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    #[cfg(target_vendor = "apple")]
    {
        addr.sin_len = size_of_val(&addr) as u8;
    }
    addr.sin_family = libc::AF_INET as _;
    addr.sin_port = port.to_be();
    addr.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
    Ok(addr)
}

/// # Safety
/// The operand is a rooted tuple with the exact fields (str, int).
unsafe fn unpack(pair: *const usize, name: &str) -> Result<libc::sockaddr_in> {
    // SAFETY: the compiler verified and rooted both fields.
    unsafe { address(&**pair.cast::<*const Str>(), *pair.add(1), name) }
}

/// # Safety
/// The operand is a rooted exact (str, int) address tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_bind(s: &Object, pair: *const usize) {
    // SAFETY: unpack borrows the declared fields and bind reads our stack sockaddr.
    finish(
        unsafe {
            unpack(pair, "bind").and_then(|a| {
                checked(libc::bind(
                    fd(s),
                    (&raw const a).cast(),
                    size_of_val(&a) as _,
                ))
                .map_err(errno_error)
            })
        },
        0,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_listen(s: &Object, backlog: usize) {
    finish(
        c_int(backlog).and_then(|n| {
            // SAFETY: listen takes only scalar arguments.
            checked(unsafe { libc::listen(fd(s), n.max(0)) }).map_err(errno_error)
        }),
        0,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_shutdown(s: &Object, how: usize) {
    finish(
        c_int(how).and_then(|how| {
            // SAFETY: shutdown takes scalar values and never relinquishes the descriptor.
            checked(unsafe { libc::shutdown(fd(s), how) }).map_err(errno_error)
        }),
        0,
    );
}

fn connect(fd: RawFd, addr: &libc::sockaddr_in, blocking: bool) -> io::Result<()> {
    // SAFETY: the kernel reads the supplied stack IPv4 address.
    let result = checked(unsafe {
        libc::connect(
            fd,
            (addr as *const libc::sockaddr_in).cast(),
            size_of_val(addr) as _,
        )
    });
    match result {
        Err(e) if e.kind() == io::ErrorKind::Interrupted && blocking => {}
        other => return other.map(|_| ()),
    }
    // An interrupted blocking connect is already in progress. Calling connect again
    // can report EISCONN or start a different attempt. Observe the existing attempt.
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: poll writes one live pollfd in place.
    retry(|| checked(unsafe { libc::poll(&raw mut pfd, 1, -1) }))?;
    // Retry the completion check itself on EINTR, without reconnecting.
    retry(|| {
        let mut error: i32 = 0;
        let mut len = size_of_val(&error) as libc::socklen_t;
        // SAFETY: SO_ERROR fills one int and its initialized capacity.
        checked(unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &raw mut len,
            )
        })?;
        if error == 0 || error == libc::EISCONN {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    })
}

/// # Safety
/// The operand is a rooted exact (str, int) address tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_connect(s: &Object, pair: *const usize) {
    // SAFETY: the call retains the exact address tuple.
    finish(
        unsafe { unpack(pair, "connect") }
            .and_then(|a| connect(fd(s), &a, state(s, |s| s.blocking)).map_err(errno_error)),
        (),
    );
}

// Caller supplies the generated tuple descriptor so repr, projection and release
// use precisely the same layout as a tuple literal.
unsafe fn addr_tuple(layout: *const RecordDescriptor, addr: libc::sockaddr_in) -> *mut usize {
    let host = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
    let host = crate::StrOwner::new(Str::from(host.to_string()));
    // SAFETY: the descriptor describes (str, int); initialize and transfer both owners.
    unsafe {
        let pair = imre_record_new(layout);
        pair.cast::<*const Str>()
            .write(crate::StrOwner::into_raw(host));
        pair.add(1)
            .write(word(i32::from(u16::from_be(addr.sin_port))));
        pair
    }
}

fn peer_call(
    mut call: impl FnMut(*mut libc::sockaddr, *mut libc::socklen_t) -> i32,
) -> io::Result<(i32, libc::sockaddr_in)> {
    // SAFETY: all fields have a valid zero representation before the kernel writes them.
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut len = size_of_val(&addr) as libc::socklen_t;
    let ret = checked(call((&raw mut addr).cast(), &raw mut len))?;
    Ok((ret, addr))
}

/// # Safety
/// The result descriptor is the static layout of (str, int).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_sockname(
    s: &Object,
    layout: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: peer_call supplies a writable sockaddr_in and its capacity; layout matches.
    let result = unsafe {
        peer_call(|a, n| libc::getsockname(fd(s), a, n)).map(|(_, a)| addr_tuple(layout, a))
    };
    finish(result.map_err(errno_error), ptr::null_mut())
}

/// # Safety
/// Static descriptors describe (socket, (str, int)) and (str, int), respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_accept(
    s: &Object,
    layout: *const RecordDescriptor,
    addr_layout: *const RecordDescriptor,
) -> *mut usize {
    let raw = fd(s);
    let result = retry(|| {
        peer_call(|a, n| {
            // SAFETY: peer_call provides a fresh address and resets its capacity on every attempt.
            unsafe {
                #[cfg(target_os = "linux")]
                {
                    libc::accept4(raw, a, n, libc::SOCK_CLOEXEC)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    libc::accept(raw, a, n)
                }
            }
        })
    })
    .and_then(|(raw, a)| {
        let child = handle(raw, true)?;
        // SAFETY: both descriptors match these transferred owners. No fallible syscall remains.
        unsafe {
            let pair = imre_record_new(layout);
            pair.cast::<*const Object>()
                .write(crate::ObjectOwner::into_raw(child));
            pair.add(1)
                .cast::<*mut usize>()
                .write(addr_tuple(addr_layout, a));
            Ok(pair)
        }
    });
    finish(result.map_err(errno_error), ptr::null_mut())
}

/// # Safety
/// Size is a rooted Python int word for the duration of conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_recv(s: &Object, size: usize, flags: usize) -> *const Bytes {
    let result = (|| {
        // SAFETY: the caller roots the int; as_small avoids allocating on the common path.
        let size = unsafe { Int::borrow(size) };
        let size = size
            .as_small()
            .or_else(|| size.big().to_i64())
            .ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    "Python int too large to convert to C ssize_t",
                )
            })?;
        let flags = c_int(flags)?;
        let size = usize::try_from(size)
            .map_err(|_| Error::new(Kind::ValueError, "negative buffersize in recv"))?;
        let mut bytes = Vec::<u8>::new();
        if size != 0 {
            bytes
                .try_reserve_exact(size)
                .map_err(|_| Error::allocation())?;
            let raw = fd(s);
            let n = retry(|| {
                // SAFETY: the Vec owns size writable bytes; no uninitialized byte is read.
                let n = unsafe { libc::recv(raw, bytes.as_mut_ptr().cast(), size, flags) };
                usize::try_from(n).map_err(|_| io::Error::last_os_error())
            })
            .map_err(errno_error)?;
            // SAFETY: recv initialized exactly n bytes of private storage.
            unsafe {
                bytes.set_len(n);
            }
            if n < size / 2 {
                bytes.shrink_to_fit();
            }
        }
        crate::BytesOwner::try_new(bytes)
            .map(crate::BytesOwner::into_raw)
            .map_err(|_| Error::memory_error())
    })();
    finish(result, ptr::null())
}

fn send(raw: RawFd, bytes: &[u8], flags: i32) -> Result<usize> {
    #[cfg(not(target_vendor = "apple"))]
    let flags = flags | libc::MSG_NOSIGNAL;
    retry(|| {
        // SAFETY: the slice stays initialized and immovable across retries.
        let n = unsafe { libc::send(raw, bytes.as_ptr().cast(), bytes.len(), flags) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    })
    .map_err(errno_error)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_send(s: &Object, data: &Bytes, flags: usize) -> usize {
    finish(
        c_int(flags)
            .and_then(|flags| send(fd(s), data, flags))
            .map(|n| Int::from_usize(n).into_raw()),
        word(0),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_sendall(s: &Object, data: &Bytes, flags: usize) {
    let result = (|| {
        let flags = c_int(flags)?;
        let raw = fd(s);
        let mut remaining = data.as_slice();
        // Empty input still makes one send: EBADF/EPIPE are observable.
        loop {
            let written = send(raw, remaining, flags)?;
            remaining = &remaining[written..];
            if remaining.is_empty() {
                return Ok(());
            }
            if written == 0 {
                return Err(errno_error(io::Error::from_raw_os_error(libc::EPIPE)));
            }
        }
    })();
    finish(result, ());
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_fileno(s: &Object) -> usize {
    word(fd(s))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_close(s: &Object) {
    finish(state(s, Socket::close).map_err(errno_error), ());
}

fn set_blocking(raw: RawFd, blocking: bool) -> io::Result<()> {
    // SAFETY: fcntl reads then updates only the nonblocking status flag of this fd.
    unsafe {
        let flags = checked(libc::fcntl(raw, libc::F_GETFL))?;
        checked(libc::fcntl(
            raw,
            libc::F_SETFL,
            if blocking {
                flags & !libc::O_NONBLOCK
            } else {
                flags | libc::O_NONBLOCK
            },
        ))?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_setblocking(s: &Object, flag: usize) {
    let blocking = flag != word(0);
    // CPython updates its socket timeout state even if fcntl later reports EBADF.
    state(s, |s| s.blocking = blocking);
    finish(set_blocking(fd(s), blocking).map_err(errno_error), ());
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getblocking(s: &Object) -> bool {
    state(s, |s| s.blocking)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_dup(s: &Object) -> *const Object {
    let result = (|| {
        let (raw, blocking) = state(s, |s| (s.fd, s.blocking));
        // SAFETY: the kernel validates even a closed fd; its copy is non-inheritable from creation.
        let copy = checked(unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) })?;
        let child = handle(copy, blocking)?;
        // socket.dup reapplies its source's blocking mode to the shared open file.
        set_blocking(copy, blocking)?;
        Ok(crate::ObjectOwner::into_raw(child))
    })();
    finish(result.map_err(errno_error), ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_get_inheritable(s: &Object) -> bool {
    super::os::imre_os_get_inheritable(word(fd(s))) != 0
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_set_inheritable(s: &Object, value: usize) {
    super::os::imre_os_set_inheritable(word(fd(s)), value);
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_setsockopt(s: &Object, level: usize, name: usize, value: usize) {
    finish(
        (|| {
            let level = c_int(level)?;
            let name = c_int(name)?;
            let value = c_int(value).map_err(|_| {
                Error::new(
                    Kind::TypeError,
                    "a bytes-like object is required, not 'int'",
                )
            })?;
            setopt(fd(s), level, name, value).map_err(errno_error)
        })(),
        (),
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_setsockopt_bytes(
    s: &Object,
    level: usize,
    name: usize,
    value: &Bytes,
) {
    finish(
        (|| {
            let len = libc::socklen_t::try_from(value.len())
                .map_err(|_| Error::new(Kind::OverflowError, "setsockopt option too large"))?;
            // SAFETY: the exact immutable bytes buffer remains rooted across setsockopt.
            checked(unsafe {
                libc::setsockopt(
                    fd(s),
                    c_int(level)?,
                    c_int(name)?,
                    value.as_ptr().cast(),
                    len,
                )
            })
            .map_err(errno_error)
        })(),
        0,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getsockopt(s: &Object, level: usize, name: usize) -> usize {
    finish(
        (|| {
            let mut value: i32 = 0;
            let mut len = size_of_val(&value) as libc::socklen_t;
            // SAFETY: the caller requests a scalar option; the kernel may write at most sizeof(int).
            checked(unsafe {
                libc::getsockopt(
                    fd(s),
                    c_int(level)?,
                    c_int(name)?,
                    (&raw mut value).cast(),
                    &raw mut len,
                )
            })
            .map_err(errno_error)?;
            Ok(word(value))
        })(),
        word(0),
    )
}
