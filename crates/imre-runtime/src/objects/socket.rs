//! A socket closes its descriptor on explicit close or drop, unless detached.
use super::exceptions::{Error, Kind, Result, finish};
use super::records::{RecordDescriptor, imre_record_new};
use super::{Bytes, Int, Object, Storage, StorageCell, Str, c_int, errno_error};
use num_traits::ToPrimitive;
use std::ffi::{CStr, CString};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::ptr;
use std::time::{Duration, Instant};

pub(super) struct Socket {
    family: i32,
    kind: i32,
    proto: i32,
    fd: RawFd,
    // Python socket timeouts use signed nanoseconds: -1 blocks indefinitely, 0 is nonblocking.
    timeout_ns: i64,
    readers: usize,
    close_requested: bool,
}

impl Socket {
    fn into_owner(self) -> Result<crate::ObjectOwner> {
        crate::ObjectOwner::try_new(Object(StorageCell::new(Storage::Socket(self))))
            .map_err(|_| Error::memory_error())
    }

    fn close(&mut self) -> io::Result<()> {
        self.close_requested = true;
        if self.readers != 0 {
            return Ok(());
        }
        self.close_fd()
    }

    fn close_fd(&mut self) -> io::Result<()> {
        let fd = std::mem::replace(&mut self.fd, -1);
        if fd == -1 {
            return Ok(());
        }
        close_fd(fd)
    }
}

/// Close a raw socket descriptor once, including when close reports EINTR.
pub(crate) fn close_fd(fd: RawFd) -> io::Result<()> {
    // SAFETY: close takes only a descriptor number. Retrying could close a reused fd.
    match checked(unsafe { libc::close(fd) }) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::ECONNRESET) => Ok(()),
        Err(e) => Err(e),
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = self.close_fd();
    }
}

/// The raw input for socket.makefile borrows the same descriptor until its last reader closes.
pub(crate) struct SocketInput {
    socket: Option<crate::ObjectOwner>,
    buffer: io::Cursor<Vec<u8>>,
    timed_out: bool,
}

const SOCKET_BUFFER_SIZE: usize = 128 * 1024;

impl SocketInput {
    /// Reserve the read buffer before retaining a reference to the descriptor.
    pub(crate) fn try_new(socket: crate::ObjectOwner) -> Result<Self> {
        // CPython uses io.DEFAULT_BUFFER_SIZE for the default buffered reader.
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(SOCKET_BUFFER_SIZE)
            .map_err(|_| Error::memory_error())?;
        state(&socket, |s| s.readers += 1);
        Ok(Self {
            socket: Some(socket),
            buffer: io::Cursor::new(bytes),
            timed_out: false,
        })
    }

    // SocketIO permanently refuses reads after the first read timeout on this file.
    unsafe fn read_socket(&mut self, target: *mut u8, size: usize) -> io::Result<usize> {
        if size == 0 {
            return Ok(0);
        }
        if self.timed_out {
            return Err(io::Error::other("cannot read from timed out object"));
        }
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        // SAFETY: the caller provides size writable bytes.
        let result = unsafe { receive(socket, target, size, 0) };
        if let Err(err) = &result {
            self.timed_out = err.kind() == io::ErrorKind::TimedOut;
        }
        result
    }

    /// Consume buffered input first, otherwise receive at most the requested bytes once.
    pub(crate) fn read1(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if self.buffer.position() < self.buffer.get_ref().len() as u64 {
            return io::Read::read(&mut self.buffer, target);
        }
        // SAFETY: target is writable for its length, and receive does not retain it.
        match unsafe { self.read_socket(target.as_mut_ptr(), target.len()) } {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(0),
            result => result,
        }
    }

    pub(crate) fn close(&mut self) -> io::Result<()> {
        let Some(socket) = self.socket.take() else {
            return Ok(());
        };
        state(&socket, |s| {
            s.readers -= 1;
            if s.close_requested && s.readers == 0 {
                s.close_fd()
            } else {
                Ok(())
            }
        })
    }
}

impl AsRawFd for SocketInput {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_ref().map_or(-1, |s| fd(s))
    }
}

impl io::Read for SocketInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.buffer.position() >= self.buffer.get_ref().len() as u64
            && buffer.len() >= SOCKET_BUFFER_SIZE
        {
            // SAFETY: buffer exposes this many exclusively writable bytes.
            return unsafe { self.read_socket(buffer.as_mut_ptr(), buffer.len()) };
        }
        let count = io::Read::read(&mut io::BufRead::fill_buf(self)?, buffer)?;
        io::BufRead::consume(self, count);
        Ok(count)
    }
}

impl io::BufRead for SocketInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.buffer.position() >= self.buffer.get_ref().len() as u64 {
            self.buffer.get_mut().clear();
            let target = self.buffer.get_mut().as_mut_ptr();
            // SAFETY: try_new reserved this many bytes, and recv initializes the returned prefix.
            let count = unsafe { self.read_socket(target, SOCKET_BUFFER_SIZE)? };
            // SAFETY: receive initialized exactly count bytes in the reserved buffer.
            unsafe { self.buffer.get_mut().set_len(count) };
            self.buffer.set_position(0);
        }
        self.buffer.fill_buf()
    }

    fn consume(&mut self, count: usize) {
        self.buffer.consume(count);
    }
}

impl Drop for SocketInput {
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

pub(crate) fn family(s: &Object) -> i32 {
    state(s, |s| s.family)
}

pub(crate) fn kind(s: &Object) -> i32 {
    state(s, |s| s.kind)
}

pub(crate) fn proto(s: &Object) -> i32 {
    state(s, |s| s.proto)
}

pub(crate) fn fd(s: &Object) -> RawFd {
    state(s, |s| s.fd)
}

/// Move a blocking stream to its TLS owner and report whether a handshake can begin.
#[cfg(unix)]
pub(crate) fn take_tls_stream(socket: &Object, server: bool) -> Result<(crate::ObjectOwner, bool)> {
    if kind(socket) != libc::SOCK_STREAM {
        return Err(Error::new(
            Kind::NotImplementedError,
            "only stream sockets are supported",
        ));
    }
    let connected = match peer_call(|addr, len| {
        // SAFETY: peer_call provides writable storage and its capacity for getpeername.
        unsafe { libc::getpeername(fd(socket), addr, len) }
    }) {
        Ok(_) => true,
        Err(e) if server && e.raw_os_error() == Some(libc::ENOTCONN) => false,
        Err(e) => return Err(errno_error(e)),
    };
    state(socket, |s| {
        if s.close_requested || s.readers != 0 {
            return Err(Error::new(
                Kind::ValueError,
                "cannot wrap a closing socket or one with active file readers",
            ));
        }
        // SAFETY: this query only reads the flags of the socket-owned descriptor.
        let flags = checked(unsafe { libc::fcntl(s.fd, libc::F_GETFL) }).map_err(errno_error)?;
        if s.timeout_ns >= 0 || flags & libc::O_NONBLOCK != 0 {
            return Err(Error::new(
                Kind::NotImplementedError,
                "TLS requires a blocking socket without a timeout",
            ));
        }
        let transport = Socket {
            fd: std::mem::replace(&mut s.fd, -1),
            ..*s
        };
        s.close_requested = true;
        Ok((transport.into_owner()?, connected))
    })
}

/// Detect cleartext queued on a peer that closed before TLS started.
#[cfg(unix)]
pub(crate) fn pending_tls_data(socket: &Object) -> Result<bool> {
    let mut byte = 0;
    // SAFETY: byte is writable for this call. The transferred blocking socket has no timeout.
    match unsafe { receive(socket, &raw mut byte, 1, libc::MSG_DONTWAIT) } {
        Ok(n) => Ok(n != 0),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTCONN | libc::EINVAL)) => Ok(false),
        Err(e) => Err(errno_error(e)),
    }
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

/// Return a single deadline for an entire socket operation, including retries and sendall.
#[derive(Clone, Copy)]
struct Deadline {
    start: Instant,
    duration: Duration,
}

impl Deadline {
    fn remaining(self) -> Duration {
        self.duration.saturating_sub(self.start.elapsed())
    }
}

fn deadline(socket: &Object) -> Option<Deadline> {
    get_timeout(socket)
        .filter(|d| !d.is_zero())
        .map(|duration| Deadline {
            start: Instant::now(),
            duration,
        })
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timed out")
}

/// Wait without extending the deadline on a signal, spurious readiness or a long poll interval.
fn poll_socket(fd: RawFd, events: i16, until: Option<Deadline>) -> io::Result<()> {
    let mut pollfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        let timeout = until.map_or(-1, |end| {
            end.remaining()
                .as_nanos()
                .div_ceil(1_000_000)
                .min(i32::MAX as u128) as i32
        });
        // SAFETY: poll may write revents on this one live descriptor record.
        match checked(unsafe { libc::poll(&raw mut pollfd, 1, timeout) }) {
            Ok(n) if n > 0 => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
        if until.is_some_and(|end| end.remaining().is_zero()) {
            return Err(timed_out());
        }
    }
}

/// Wait for the direction requested by a blocking TLS operation, retrying interrupted polls.
#[cfg(unix)]
pub(crate) fn wait_blocking(fd: RawFd, events: i16) -> io::Result<()> {
    poll_socket(fd, events, None)
}

/// Issue a nonblocking syscall and retry on readiness only when a timeout was requested.
fn socket_call<T>(
    fd: RawFd,
    event: i16,
    until: Option<Deadline>,
    mut call: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    loop {
        // A negative fd is ignored by poll; the syscall itself must report EBADF.
        if fd >= 0 && until.is_some() {
            poll_socket(fd, event, until)?;
        }
        match call() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if until.is_some() && e.kind() == io::ErrorKind::WouldBlock => {}
            result => return result,
        }
        if until.is_some_and(|end| end.remaining().is_zero()) {
            return Err(timed_out());
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
    imre_socket_so_type = libc::SO_TYPE,
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

fn handle(
    raw: RawFd,
    blocking: bool,
    family: i32,
    kind: i32,
    proto: i32,
) -> Result<crate::ObjectOwner> {
    // SAFETY: the kernel transferred a new fd; every path now has an owner.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: the fresh descriptor is exclusively owned until setup completes.
        checked(unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) })
            .map_err(errno_error)?;
    }
    #[cfg(target_vendor = "apple")]
    setopt(raw, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1).map_err(errno_error)?;
    let socket = socket_state(family, kind, proto, blocking)?;
    state(&socket, |s| s.fd = fd.into_raw_fd());
    Ok(socket)
}

fn socket_state(family: i32, kind: i32, proto: i32, blocking: bool) -> Result<crate::ObjectOwner> {
    Socket {
        fd: -1,
        family,
        kind,
        proto,
        timeout_ns: if blocking { -1 } else { 0 },
        readers: 0,
        close_requested: false,
    }
    .into_owner()
}

/// Adopt the caller’s descriptor after validating it and filling omitted metadata.
pub(crate) fn adopt(
    raw: RawFd,
    mut family: i32,
    mut kind: i32,
    mut proto: i32,
) -> Result<crate::ObjectOwner> {
    if raw < 0 {
        return Err(Error::new(Kind::ValueError, "negative file descriptor"));
    }
    match peer_call(|addr, len| {
        // SAFETY: peer_call supplies writable sockaddr storage and its capacity.
        unsafe { libc::getsockname(raw, addr, len) }
    }) {
        Ok((_, addr, _)) if family == -1 => family = i32::from(addr.ss_family),
        Err(e)
            if family == -1 || matches!(e.raw_os_error(), Some(libc::EBADF | libc::ENOTSOCK)) =>
        {
            return Err(errno_error(e));
        }
        // Some valid families (e.g. AF_ALG) do not implement getsockname.
        _ => {}
    }
    if kind == -1 {
        kind = int_option(raw, libc::SOL_SOCKET, libc::SO_TYPE)?;
    }
    #[cfg(target_os = "linux")]
    if proto == -1 {
        proto = int_option(raw, libc::SOL_SOCKET, libc::SO_PROTOCOL)?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        proto = 0;
    }
    #[cfg(target_os = "linux")]
    let (kind, blocking) = (
        kind & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC),
        kind & libc::SOCK_NONBLOCK == 0,
    );
    #[cfg(not(target_os = "linux"))]
    let blocking = true;
    let socket = socket_state(family, kind, proto, blocking)?;
    state(&socket, |s| s.fd = raw);
    Ok(socket)
}

/// Return the underlying descriptor and disable subsequent closes by this socket.
pub(crate) fn detach(socket: &Object) -> RawFd {
    state(socket, |s| {
        s.close_requested = true;
        std::mem::replace(&mut s.fd, -1)
    })
}

pub(crate) fn open(family: i32, ty: i32, proto: i32) -> Result<crate::ObjectOwner> {
    // socket.socket uses -1 to request the default for each field.
    let family = if family == -1 { libc::AF_INET } else { family };
    let ty = if ty == -1 { libc::SOCK_STREAM } else { ty };
    let proto = if proto == -1 { 0 } else { proto };
    if !matches!(family, libc::AF_INET | libc::AF_INET6 | libc::AF_UNIX)
        || !matches!(ty, libc::SOCK_STREAM | libc::SOCK_DGRAM)
    {
        return Err(Error::new(
            Kind::NotImplementedError,
            "only Internet and Unix stream and datagram sockets are supported",
        ));
    }
    let kind = ty;
    #[cfg(target_os = "linux")]
    let ty = ty | libc::SOCK_CLOEXEC;
    // SAFETY: socket takes only scalars, and handle acquires the fresh descriptor.
    let raw = checked(unsafe { libc::socket(family, ty, proto) }).map_err(errno_error)?;
    handle(raw, true, family, kind, proto)
}

pub(crate) fn socketpair(
    family: i32,
    kind: i32,
    proto: i32,
) -> Result<(crate::ObjectOwner, crate::ObjectOwner)> {
    #[cfg(target_os = "linux")]
    let (flags, blocking, kind) = (
        kind | libc::SOCK_CLOEXEC,
        kind & libc::SOCK_NONBLOCK == 0,
        kind & !(libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK),
    );
    #[cfg(not(target_os = "linux"))]
    let (flags, blocking) = (kind, true);

    let mut pair = [-1; 2];
    // SAFETY: on success socketpair initializes both entries and transfers both descriptors.
    checked(unsafe { libc::socketpair(family, flags, proto, pair.as_mut_ptr()) })
        .map_err(errno_error)?;
    // SAFETY: the second descriptor needs an owner even if setting up the first endpoint fails.
    let right = unsafe { OwnedFd::from_raw_fd(pair[1]) };
    let left = handle(pair[0], blocking, family, kind, proto)?;
    let right = handle(right.into_raw_fd(), blocking, family, kind, proto)?;
    Ok((left, right))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_new(family: usize, ty: usize, proto: usize) -> *const Object {
    finish(
        (|| {
            let family = c_int(family)?;
            if matches!(family, libc::AF_INET6 | libc::AF_UNIX) {
                return Err(Error::new(
                    Kind::NotImplementedError,
                    "typed socket addresses require AF_INET",
                ));
            }
            open(family, c_int(ty)?, c_int(proto)?)
        })()
        .map(crate::ObjectOwner::into_raw),
        ptr::null(),
    )
}

/// Resolve one Internet address with CPython's wildcard and broadcast rules.
fn lookup_ip(host: &[u8], family: i32) -> Result<IpAddr> {
    if host.is_empty() && family == libc::AF_INET {
        return Ok(Ipv4Addr::UNSPECIFIED.into());
    }
    if host == b"<broadcast>" {
        return Ok(Ipv4Addr::BROADCAST.into());
    }
    if let Ok(text) = std::str::from_utf8(host)
        && let Ok(ip) = text.parse::<IpAddr>()
        && (family == libc::AF_UNSPEC || ip.is_ipv4())
    {
        return Ok(ip);
    }
    let list = if host.is_empty() {
        address_list(
            None,
            Some(b"0"),
            family,
            libc::SOCK_DGRAM,
            0,
            libc::AI_PASSIVE,
        )?
    } else {
        address_list(Some(host), None, family, 0, 0, 0)?
    };
    // SAFETY: address_list returns a non-null owned list with initialized address lengths.
    let addr = unsafe {
        let first = &*list.0;
        if host.is_empty() && !first.ai_next.is_null() {
            return Err(Error::new(
                Kind::OSError,
                "wildcard resolved to multiple address",
            ));
        }
        InternetAddress::read(first.ai_addr, first.ai_addrlen)?
    };
    Ok(match addr {
        InternetAddress::V4(v4) => endpoint(&v4).0.into(),
        InternetAddress::V6(v6) => Ipv6Addr::from(v6.sin6_addr.s6_addr).into(),
    })
}

pub(crate) fn hostbyname(host: &[u8]) -> Result<Ipv4Addr> {
    match lookup_ip(host, libc::AF_INET)? {
        IpAddr::V4(ip) => Ok(ip),
        IpAddr::V6(_) => Err(Error::new(Kind::OSError, "address family mismatched")),
    }
}

pub(crate) struct AddressInfo {
    pub(crate) ty: i32,
    pub(crate) protocol: i32,
    pub(crate) canonical: String,
    pub(crate) addr: InternetAddress,
}

struct AddrInfoList(*mut libc::addrinfo);

impl Drop for AddrInfoList {
    fn drop(&mut self) {
        // SAFETY: the list was returned by getaddrinfo and remains uniquely owned.
        unsafe { libc::freeaddrinfo(self.0) };
    }
}

fn check_gai(code: i32) -> Result<()> {
    if code == libc::EAI_SYSTEM {
        return Err(errno_error(io::Error::last_os_error()));
    }
    if code != 0 {
        // SAFETY: gai_strerror returns a static terminated message for a resolver error code.
        let message = unsafe { CStr::from_ptr(libc::gai_strerror(code)) }.to_string_lossy();
        return Err(Error::from_errno(
            Kind::GaiError,
            code,
            &message,
            None,
            None,
        ));
    }
    Ok(())
}

fn address_list(
    host: Option<&[u8]>,
    service: Option<&[u8]>,
    family: i32,
    ty: i32,
    protocol: i32,
    flags: i32,
) -> Result<AddrInfoList> {
    let text = |value: &[u8]| {
        CString::new(value).map_err(|_| {
            Error::new(
                Kind::TypeError,
                "host or service must not contain null character",
            )
        })
    };
    let host = host.map(text).transpose()?;
    let service = service.map(text).transpose()?;
    // SAFETY: zeros initialize the optional pointers and flags in the hints.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = family;
    hints.ai_socktype = ty;
    hints.ai_protocol = protocol;
    hints.ai_flags = flags;
    let mut res = ptr::null_mut();
    // SAFETY: host is terminated, hints is initialized, and res is an out pointer.
    let code = unsafe {
        libc::getaddrinfo(
            host.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            service.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            &raw const hints,
            &raw mut res,
        )
    };
    check_gai(code)?;
    if res.is_null() {
        return Err(Error::new(
            Kind::OSError,
            "getaddrinfo returned an empty list",
        ));
    }
    Ok(AddrInfoList(res))
}

pub(crate) fn getaddrinfo(
    host: Option<&[u8]>,
    service: Option<&[u8]>,
    family: i32,
    ty: i32,
    protocol: i32,
    flags: i32,
) -> Result<Vec<AddressInfo>> {
    let list = address_list(host, service, family, ty, protocol, flags)?;
    let mut cursor = list.0;
    let mut rows = Vec::new();
    // SAFETY: all nodes and canonical names remain live until the list guard is dropped.
    while let Some(entry) = unsafe { cursor.as_ref() } {
        // SAFETY: the list guard owns the address and libc supplies its readable length.
        let addr = unsafe { InternetAddress::read(entry.ai_addr, entry.ai_addrlen)? };
        if entry.ai_family != addr.family() {
            return Err(Error::new(
                Kind::OSError,
                "resolver returned an inconsistent address family",
            ));
        }
        rows.try_reserve(1).map_err(|_| Error::memory_error())?;
        let canonical = if entry.ai_canonname.is_null() {
            String::new()
        } else {
            // SAFETY: libc supplies a terminated canonical name on this node.
            unsafe { CStr::from_ptr(entry.ai_canonname) }
                .to_string_lossy()
                .into_owned()
        };
        rows.push(AddressInfo {
            ty: entry.ai_socktype,
            protocol: entry.ai_protocol,
            canonical,
            addr,
        });
        cursor = entry.ai_next;
    }
    Ok(rows)
}

/// Resolve the numeric sockaddr first, then let the platform interpret the requested NI flags.
pub(crate) fn getnameinfo(
    host: &[u8],
    port: i32,
    flowinfo: Option<u32>,
    scope_id: u32,
    flags: i32,
) -> Result<(String, String)> {
    let list = address_list(
        Some(host),
        Some(port.to_string().as_bytes()),
        libc::AF_UNSPEC,
        libc::SOCK_DGRAM,
        0,
        libc::AI_NUMERICHOST,
    )?;
    // SAFETY: address_list returned a non-null list owned until the end of this function.
    let entry = unsafe { &*list.0 };
    if !entry.ai_next.is_null() {
        return Err(Error::new(
            Kind::OSError,
            "sockaddr resolved to multiple addresses",
        ));
    }
    // SAFETY: the list guard keeps ai_addr readable for its reported length.
    let mut addr = unsafe { InternetAddress::read(entry.ai_addr, entry.ai_addrlen)? };
    match &mut addr {
        InternetAddress::V4(_) if flowinfo.is_some() => {
            return Err(Error::new(Kind::OSError, "IPv4 sockaddr must be 2 tuple"));
        }
        InternetAddress::V6(v6) => {
            v6.sin6_flowinfo = flowinfo.unwrap_or(0).to_be();
            v6.sin6_scope_id = scope_id;
        }
        InternetAddress::V4(_) => {}
    }
    let (raw, len) = addr.raw_parts();
    let mut host = [0; libc::NI_MAXHOST as usize];
    let mut service = [0; 32]; // NI_MAXSERV on supported platforms.
    // SAFETY: addr lives through the call and both writable lengths match their buffers.
    check_gai(unsafe {
        libc::getnameinfo(
            raw,
            len,
            host.as_mut_ptr(),
            host.len() as _,
            service.as_mut_ptr(),
            service.len() as _,
            flags,
        )
    })?;
    let decode = |buffer: &[libc::c_char]| {
        // SAFETY: getnameinfo succeeded, so both output buffers are NUL terminated.
        let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|err| super::bytes::decode_error(bytes, err))
    };
    Ok((decode(&host)?, decode(&service)?))
}

enum Service<'a> {
    Name(&'a CStr),
    Port(u16),
}

/// Copy the requested field while the platform's service entry is still valid.
fn with_service<T>(
    key: Service<'_>,
    protocol: Option<&CStr>,
    read: impl FnOnce(&libc::servent) -> Result<T>,
) -> Result<T> {
    let proto = protocol.map_or(ptr::null(), CStr::as_ptr);
    let missing = match key {
        Service::Name(_) => "service/proto not found",
        Service::Port(_) => "port/proto not found",
    };

    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn getservbyname_r(
                name: *const libc::c_char,
                proto: *const libc::c_char,
                entry: *mut libc::servent,
                buffer: *mut libc::c_char,
                size: usize,
                result: *mut *mut libc::servent,
            ) -> libc::c_int;
            fn getservbyport_r(
                port: libc::c_int,
                proto: *const libc::c_char,
                entry: *mut libc::servent,
                buffer: *mut libc::c_char,
                size: usize,
                result: *mut *mut libc::servent,
            ) -> libc::c_int;
        }

        let mut entry = std::mem::MaybeUninit::uninit();
        // NSS can need more space for aliases. Keep its scratch storage pointer-aligned.
        let mut buffer = Vec::<usize>::new();
        let mut words = 1024 / size_of::<usize>();
        loop {
            buffer
                .try_reserve_exact(words)
                .map_err(|_| Error::memory_error())?;
            let bytes = buffer.capacity() * size_of::<usize>();
            let buf = buffer.as_mut_ptr().cast();
            let mut result = ptr::null_mut();
            // SAFETY: both C strings are live or NULL, and the output and scratch storage
            // remain valid until the requested field has been copied.
            let rc = unsafe {
                match key {
                    Service::Name(name) => getservbyname_r(
                        name.as_ptr(),
                        proto,
                        entry.as_mut_ptr(),
                        buf,
                        bytes,
                        &raw mut result,
                    ),
                    Service::Port(port) => getservbyport_r(
                        i32::from(port.to_be()),
                        proto,
                        entry.as_mut_ptr(),
                        buf,
                        bytes,
                        &raw mut result,
                    ),
                }
            };
            if rc == libc::ERANGE {
                words = buffer
                    .capacity()
                    .checked_mul(2)
                    .ok_or_else(Error::memory_error)?;
                continue;
            }
            if rc != 0 || result.is_null() {
                return Err(Error::new(Kind::OSError, missing));
            }
            // SAFETY: a successful lookup initialized this entry in our live storage.
            return read(unsafe { &*result });
        }
    }
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: Darwin keeps the returned item in thread-local storage. Copy the field
        // before another service lookup can replace it; neither reader calls back into Python.
        unsafe {
            let result = match key {
                Service::Name(name) => libc::getservbyname(name.as_ptr(), proto),
                Service::Port(port) => libc::getservbyport(i32::from(port.to_be()), proto),
            };
            read(
                result
                    .as_ref()
                    .ok_or_else(|| Error::new(Kind::OSError, missing))?,
            )
        }
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    Err(Error::new(
        Kind::NotImplementedError,
        "service lookup requires Linux or Darwin",
    ))
}

pub(crate) fn service_by_name(name: &CStr, protocol: Option<&CStr>) -> Result<u16> {
    with_service(Service::Name(name), protocol, |entry| {
        Ok(u16::from_be(entry.s_port as u16))
    })
}

pub(crate) fn service_by_port(port: u16, protocol: Option<&CStr>) -> Result<String> {
    with_service(Service::Port(port), protocol, |entry| {
        // SAFETY: the service entry owns a NUL-terminated canonical name.
        let name = unsafe { CStr::from_ptr(entry.s_name) }.to_bytes();
        std::str::from_utf8(name)
            .map(str::to_owned)
            .map_err(|err| super::bytes::decode_error(name, err))
    })
}

/// Darwin's mDNS resolver also puts the owner of the PTR query in h_aliases.
/// Only exclude the owner of this address, preserving other dotted aliases.
#[cfg(any(target_vendor = "apple", test))]
fn is_reverse_owner(addr: std::net::IpAddr, alias: &str) -> bool {
    use std::fmt::Write as _;
    let alias = alias.strip_suffix('.').unwrap_or(alias);
    if !alias
        .rsplit('.')
        .next()
        .is_some_and(|label| label.eq_ignore_ascii_case("arpa"))
    {
        return false;
    }
    let owner = match addr {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        std::net::IpAddr::V6(ip) => {
            let mut owner = String::with_capacity(72);
            for byte in ip.octets().into_iter().rev() {
                write!(owner, "{:x}.{:x}.", byte & 15, byte >> 4).unwrap();
            }
            owner.push_str("ip6.arpa");
            owner
        }
    };
    alias.eq_ignore_ascii_case(&owner)
}

pub(crate) struct HostEntry {
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) addresses: Vec<String>,
}

/// Copy a possibly unaligned, null-terminated pointer list from a live hostent.
unsafe fn host_list<T>(
    mut list: *mut *mut libc::c_char,
    mut copy: impl FnMut(*mut libc::c_char) -> Result<T>,
) -> Result<Vec<T>> {
    let mut result = Vec::new();
    if !list.is_null() {
        loop {
            // SAFETY: the caller keeps the storage live and libc terminates the list with a null entry.
            let item = unsafe { list.read_unaligned() };
            if item.is_null() {
                break;
            }
            result.push(copy(item)?);
            // SAFETY: a non-null entry is followed by at least the terminator.
            list = unsafe { list.add(1) };
        }
    }
    Ok(result)
}

/// Copy the names and address bytes while the resolver's hostent storage is live.
unsafe fn copy_host(host: *const libc::hostent, family: i32, length: i32) -> Result<HostEntry> {
    // SAFETY: the caller supplies the resolver's initialized hostent and keeps its
    // backing storage alive and exclusive until this function returns.
    unsafe {
        let Some(host) = host.as_ref() else {
            unsafe extern "C" {
                fn imre_socket_host_errno() -> libc::c_int;
            }
            // CPython's gethost_common reads h_errno even when gethostbyaddr_r supplied
            // a separate error code. Read it before calling another libc function.
            let code = imre_socket_host_errno();
            let msg = CStr::from_ptr(libc::hstrerror(code)).to_string_lossy();
            return Err(Error::from_errno(Kind::HError, code, &msg, None, None));
        };
        if host.h_addrtype != family || host.h_length != length || host.h_name.is_null() {
            return Err(errno_error(io::Error::from_raw_os_error(
                libc::EAFNOSUPPORT,
            )));
        }
        let name = |s: *mut libc::c_char| {
            let bytes = CStr::from_ptr(s).to_bytes();
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|err| super::bytes::decode_error(bytes, err))
        };
        Ok(HostEntry {
            name: name(host.h_name)?,
            aliases: host_list(host.h_aliases, name)?,
            addresses: host_list(host.h_addr_list, |p| {
                if family == libc::AF_INET {
                    Ok(Ipv4Addr::from(p.cast::<[u8; 4]>().read_unaligned()).to_string())
                } else {
                    ipv6_text(&p.cast::<libc::in6_addr>().read_unaligned())
                }
            })?,
        })
    }
}

fn reverse_host(addr: IpAddr) -> Result<HostEntry> {
    let (bytes, family): (&[u8], _) = match addr {
        IpAddr::V4(ip) => (&ip.octets(), libc::AF_INET),
        IpAddr::V6(ip) => (&ip.octets(), libc::AF_INET6),
    };
    let length = bytes.len() as i32;
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn gethostbyaddr_r(
                addr: *const libc::c_void,
                len: libc::socklen_t,
                family: libc::c_int,
                entry: *mut libc::hostent,
                buffer: *mut libc::c_char,
                buflen: usize,
                result: *mut *mut libc::hostent,
                host_error: *mut libc::c_int,
            ) -> libc::c_int;
        }
        // CPython supplies 16 KiB of aligned storage minus one byte in a single call.
        let mut buffer = [0u64; 16384 / size_of::<u64>()];
        let mut entry = std::mem::MaybeUninit::uninit();
        let mut result = ptr::null_mut();
        let mut host_error = 0;
        // SAFETY: the caller-owned buffers remain live while copy_host reads the result.
        unsafe {
            gethostbyaddr_r(
                bytes.as_ptr().cast(),
                length as _,
                family,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer) - 1,
                &raw mut result,
                &raw mut host_error,
            );
            copy_host(result, family, length)
        }
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn gethostbyaddr(
                addr: *const libc::c_void,
                len: libc::socklen_t,
                family: libc::c_int,
            ) -> *mut libc::hostent;
        }
        static REVERSE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = REVERSE.lock().unwrap_or_else(|err| err.into_inner());
        // SAFETY: the input bytes live through the call. Darwin returns per-thread
        // resolver storage; copy it before another legacy lookup on this thread.
        unsafe {
            let result = gethostbyaddr(bytes.as_ptr().cast(), length as _, family);
            copy_host(result, family, length)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(Error::new(
            Kind::NotImplementedError,
            "reverse host lookup requires Linux or macOS",
        ))
    }
}

pub(crate) fn hostbyaddr(host: &[u8]) -> Result<HostEntry> {
    reverse_host(lookup_ip(host, libc::AF_UNSPEC)?)
}

pub(crate) fn hostname() -> Result<Vec<u8>> {
    let mut buffer = [0u8; 1024];
    let n = buffer.len() - 1;
    // SAFETY: the buffer has n writable bytes and a trailing zero even if truncated.
    checked(unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), n) }).map_err(errno_error)?;
    Ok(CStr::from_bytes_until_nul(&buffer)
        .unwrap()
        .to_bytes()
        .to_vec())
}

pub(crate) fn fqdn(name: &str) -> Result<String> {
    let name = match name.trim() {
        "" | "0.0.0.0" | "::" => String::from_utf8_lossy(&hostname()?).into_owned(),
        host => host.to_owned(),
    };
    if name.as_bytes().contains(&0) {
        return Err(Error::new(
            Kind::TypeError,
            "gethostbyaddr() argument 1 must be encoded string without null bytes",
        ));
    }
    let entry = lookup_ip(name.as_bytes(), libc::AF_UNSPEC)
        .and_then(|ip| reverse_host(ip).map(|host| (ip, host)));
    let (_ip, entry) = match entry {
        Ok(result) => result,
        Err(e) if e.is_os_error() => return Ok(name),
        Err(e) => return Err(e),
    };
    if !entry.name.contains('.') {
        for alias in entry.aliases {
            #[cfg(target_vendor = "apple")]
            if is_reverse_owner(_ip, &alias) {
                continue;
            }
            if alias.contains('.') {
                return Ok(alias);
            }
        }
    }
    Ok(entry.name)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getfqdn(name: &Str) -> *const Str {
    finish(
        fqdn(name).map(|name| crate::StrOwner::into_raw(crate::StrOwner::new(Str::from(name)))),
        ptr::null(),
    )
}

fn port_error(name: &str) -> Error {
    Error::new(
        Kind::OverflowError,
        format!("{name}(): port must be 0-65535."),
    )
}

/// An address passed to bind, connect or sendto, including the exact Unix address length.
pub(crate) enum SocketAddress {
    Internet(InternetAddress),
    Unix(libc::sockaddr_un, libc::socklen_t),
}

impl SocketAddress {
    pub(crate) fn unix(path: &[u8]) -> Result<Self> {
        // SAFETY: sockaddr_un consists of integers and a byte array, including on BSD.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as _;
        // On Linux, empty input requests an autobound address and abstract names are binary.
        let terminated = !(cfg!(target_os = "linux") && (path.is_empty() || path[0] == 0));
        if path.len() > addr.sun_path.len() - usize::from(terminated) {
            return Err(Error::new(Kind::OSError, "AF_UNIX path too long"));
        }
        for (dest, byte) in addr.sun_path.iter_mut().zip(path) {
            *dest = *byte as _;
        }
        let len = std::mem::offset_of!(libc::sockaddr_un, sun_path)
            + path.len()
            + usize::from(terminated);
        Ok(Self::Unix(addr, len as _))
    }

    fn raw_parts(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        match self {
            Self::Internet(addr) => addr.raw_parts(),
            Self::Unix(addr, len) => ((addr as *const libc::sockaddr_un).cast(), *len),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum InternetAddress {
    V4(libc::sockaddr_in),
    V6(libc::sockaddr_in6),
}

impl InternetAddress {
    pub(crate) fn family(&self) -> i32 {
        match self {
            Self::V4(_) => libc::AF_INET,
            Self::V6(_) => libc::AF_INET6,
        }
    }

    fn ipv4(self) -> Result<libc::sockaddr_in> {
        match self {
            Self::V4(addr) => Ok(addr),
            Self::V6(_) => Err(Error::new(
                Kind::NotImplementedError,
                "typed socket addresses require AF_INET",
            )),
        }
    }

    fn raw_parts(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        match self {
            Self::V4(v) => ((v as *const libc::sockaddr_in).cast(), size_of_val(v) as _),
            Self::V6(v) => ((v as *const libc::sockaddr_in6).cast(), size_of_val(v) as _),
        }
    }

    /// Copy a kernel or resolver address. The caller guarantees `len` readable bytes.
    unsafe fn read(addr: *const libc::sockaddr, len: libc::socklen_t) -> Result<Self> {
        let len = len as usize;
        if !addr.is_null() && len >= std::mem::offset_of!(libc::sockaddr, sa_data) {
            // SAFETY: the family is in the common header. Check its size before reading its payload.
            unsafe {
                match i32::from((*addr).sa_family) {
                    libc::AF_INET if len == size_of::<libc::sockaddr_in>() => {
                        return Ok(Self::V4(*addr.cast()));
                    }
                    libc::AF_INET6 if len == size_of::<libc::sockaddr_in6>() => {
                        return Ok(Self::V6(*addr.cast()));
                    }
                    _ => {}
                }
            }
        }
        Err(Error::new(
            Kind::NotImplementedError,
            "unsupported socket address",
        ))
    }
}

/// Format with the platform routine used by socket addresses, including compatible IPv4 forms.
pub(crate) fn ipv6_text(ip: &libc::in6_addr) -> Result<String> {
    unsafe extern "C" {
        fn inet_ntop(
            af: libc::c_int,
            src: *const libc::c_void,
            dst: *mut libc::c_char,
            size: libc::socklen_t,
        ) -> *const libc::c_char;
    }
    let mut text = [0; 46]; // INET6_ADDRSTRLEN, including the final NUL.
    // SAFETY: AF_INET6 selects the initialized in6_addr and the buffer's exact capacity.
    let ptr = unsafe {
        inet_ntop(
            libc::AF_INET6,
            (ip as *const libc::in6_addr).cast(),
            text.as_mut_ptr(),
            text.len() as _,
        )
    };
    if ptr.is_null() {
        return Err(errno_error(io::Error::last_os_error()));
    }
    // SAFETY: success writes a terminated numeric address into text.
    Ok(unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned())
}

pub(crate) fn ipv6_address(
    host: &[u8],
    port: i32,
    flowinfo: u32,
    scope_id: u32,
    name: &str,
) -> Result<InternetAddress> {
    if matches!(host, b"<broadcast>" | b"255.255.255.255") {
        return Err(Error::new(Kind::OSError, "address family mismatched"));
    }
    let host = if host.is_empty() { b"::" } else { host };
    let resolved = address_list(Some(host), None, libc::AF_INET6, libc::SOCK_DGRAM, 0, 0)?;
    // SAFETY: address_list returned a non-null owned list; read checks the node's family and size.
    let address = unsafe {
        let first = &*resolved.0;
        InternetAddress::read(first.ai_addr, first.ai_addrlen)?
    };
    let InternetAddress::V6(mut addr) = address else {
        return Err(Error::new(
            Kind::OSError,
            "resolver returned a non-IPv6 address",
        ));
    };
    let port = u16::try_from(port).map_err(|_| port_error(name))?;
    if flowinfo > 0xfffff {
        return Err(Error::new(
            Kind::OverflowError,
            format!("{name}(): flowinfo must be 0-1048575."),
        ));
    }
    addr.sin6_port = port.to_be();
    addr.sin6_flowinfo = flowinfo.to_be();
    addr.sin6_scope_id = scope_id;
    Ok(InternetAddress::V6(addr))
}

pub(crate) fn address(host: &[u8], port: i32, name: &str) -> Result<libc::sockaddr_in> {
    let ip = hostbyname(host)?;
    let port = u16::try_from(port).map_err(|_| port_error(name))?;
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
    unsafe {
        let port = c_int(*pair.add(1)).map_err(|_| port_error(name))?;
        address((&**pair.cast::<*const Str>()).as_bytes(), port, name)
    }
}

pub(crate) fn bind(s: &Object, addr: &SocketAddress) -> Result<()> {
    let (ptr, len) = addr.raw_parts();
    // SAFETY: addr owns the complete socket address through the syscall.
    checked(unsafe { libc::bind(fd(s), ptr, len) })
        .map(|_| ())
        .map_err(errno_error)
}

/// # Safety
/// The operand is a rooted exact (str, int) address tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_bind(s: &Object, pair: *const usize) {
    // SAFETY: unpack borrows the declared fields.
    finish(
        unsafe { unpack(pair, "bind") }
            .and_then(|addr| bind(s, &SocketAddress::Internet(InternetAddress::V4(addr)))),
        (),
    );
}

pub(crate) fn listen(s: &Object, backlog: i32) -> Result<()> {
    // SAFETY: listen takes only scalar arguments.
    checked(unsafe { libc::listen(fd(s), backlog.max(0)) })
        .map(|_| ())
        .map_err(errno_error)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_listen(s: &Object, backlog: usize) {
    finish(c_int(backlog).and_then(|n| listen(s, n)), ());
}

pub(crate) fn shutdown(s: &Object, how: i32) -> Result<()> {
    // SAFETY: shutdown takes scalar values and never relinquishes the descriptor.
    checked(unsafe { libc::shutdown(fd(s), how) })
        .map(|_| ())
        .map_err(errno_error)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_shutdown(s: &Object, how: usize) {
    finish(c_int(how).and_then(|n| shutdown(s, n)), ());
}

fn connect(socket: &Object, addr: &SocketAddress) -> io::Result<()> {
    let fd = fd(socket);
    let until = deadline(socket);
    let (ptr, len) = addr.raw_parts();
    // SAFETY: addr owns the complete socket address through the syscall.
    let result = checked(unsafe { libc::connect(fd, ptr, len) });
    match result {
        Err(e)
            if e.kind() == io::ErrorKind::Interrupted
                && get_timeout(socket) != Some(Duration::ZERO) => {}
        Err(e) if until.is_some() && e.raw_os_error() == Some(libc::EINPROGRESS) => {}
        other => return other.map(|_| ()),
    }
    // An interrupted blocking connect is already in progress. Calling connect again
    // can report EISCONN or start a different attempt. Observe the existing attempt.
    poll_socket(fd, libc::POLLOUT, until)?;
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
        unsafe { unpack(pair, "connect") }.and_then(|addr| {
            connect_address(s, &SocketAddress::Internet(InternetAddress::V4(addr)))
        }),
        (),
    );
}

pub(crate) fn connect_address(s: &Object, addr: &SocketAddress) -> Result<()> {
    connect(s, addr).map_err(errno_error)
}

// Caller supplies the generated tuple descriptor so repr, projection and release
// use precisely the same layout as a tuple literal.
unsafe fn addr_tuple(layout: *const RecordDescriptor, addr: libc::sockaddr_in) -> *mut usize {
    let (host, port) = endpoint(&addr);
    let host = crate::StrOwner::new(Str::from(host.to_string()));
    // SAFETY: the descriptor describes (str, int); initialize and transfer both owners.
    unsafe {
        let pair = imre_record_new(layout);
        pair.cast::<*const Str>()
            .write(crate::StrOwner::into_raw(host));
        pair.add(1).write(word(i32::from(port)));
        pair
    }
}

fn peer_call(
    mut call: impl FnMut(*mut libc::sockaddr, *mut libc::socklen_t) -> i32,
) -> io::Result<(i32, libc::sockaddr_storage, libc::socklen_t)> {
    // SAFETY: all fields have a valid zero representation before the kernel writes them.
    let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = size_of_val(&addr) as libc::socklen_t;
    let ret = checked(call((&raw mut addr).cast(), &raw mut len))?;
    Ok((ret, addr, len))
}

pub(crate) enum SocketName {
    Internet(InternetAddress),
    Unix(Vec<u8>),
    NoAddress,
}

impl SocketName {
    fn ipv4(self) -> Result<libc::sockaddr_in> {
        match self {
            Self::Internet(addr) => addr.ipv4(),
            _ => Err(Error::new(Kind::TypeError, "expected IPv4 address")),
        }
    }
}

pub(crate) fn socket_name(s: &Object, peer: bool) -> Result<SocketName> {
    let (_, addr, len) = peer_call(|a, n| {
        // SAFETY: peer_call supplies aligned address storage and its capacity for either syscall.
        unsafe {
            if peer {
                libc::getpeername(fd(s), a, n)
            } else {
                libc::getsockname(fd(s), a, n)
            }
        }
    })
    .map_err(errno_error)?;
    unpack_socket_name(&addr, len)
}

fn unpack_socket_name(addr: &libc::sockaddr_storage, len: libc::socklen_t) -> Result<SocketName> {
    if len == 0 {
        return Ok(SocketName::NoAddress);
    }
    if i32::from(addr.ss_family) == libc::AF_UNIX {
        // SAFETY: the initialized sockaddr_storage is aligned and large enough for sockaddr_un.
        let unix = unsafe { &*(addr as *const libc::sockaddr_storage).cast::<libc::sockaddr_un>() };
        let n = (len as usize).saturating_sub(std::mem::offset_of!(libc::sockaddr_un, sun_path));
        let path = &unix.sun_path[..n.min(unix.sun_path.len())];
        let path = if cfg!(target_os = "linux") && path.first() == Some(&0) {
            path
        } else {
            path.split(|&byte| byte == 0).next().unwrap_or_default()
        };
        return Ok(SocketName::Unix(
            path.iter().map(|&byte| byte as u8).collect(),
        ));
    }
    // SAFETY: the kernel initialized addr, and read validates its family and length.
    unsafe { InternetAddress::read((addr as *const libc::sockaddr_storage).cast(), len) }
        .map(SocketName::Internet)
}

pub(crate) fn endpoint(addr: &libc::sockaddr_in) -> (Ipv4Addr, u16) {
    (
        Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
        u16::from_be(addr.sin_port),
    )
}

/// # Safety
/// The result descriptor is the static layout of (str, int).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_sockname(
    s: &Object,
    layout: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the caller supplied the exact tuple layout.
    finish(
        socket_name(s, false)
            .and_then(SocketName::ipv4)
            .map(|addr| unsafe { addr_tuple(layout, addr) }),
        ptr::null_mut(),
    )
}

pub(crate) fn accept(s: &Object) -> Result<(crate::ObjectOwner, SocketName)> {
    let raw = fd(s);
    let (child, addr, len) = socket_call(raw, libc::POLLIN, deadline(s), || {
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
    .map_err(errno_error)?;
    let child = handle(child, true, family(s), kind(s), proto(s))?;
    if get_timeout(s).is_some_and(|d| !d.is_zero()) {
        // The accepted fd may inherit O_NONBLOCK on BSD, but its Python timeout defaults to None.
        set_blocking(fd(&child), true).map_err(errno_error)?;
    }
    let addr = unpack_socket_name(&addr, len)?;
    Ok((child, addr))
}

/// # Safety
/// Static descriptors describe (socket, (str, int)) and (str, int), respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_accept(
    s: &Object,
    layout: *const RecordDescriptor,
    addr_layout: *const RecordDescriptor,
) -> *mut usize {
    let result = accept(s).and_then(|(child, a)| {
        let a = a.ipv4()?;
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
    finish(result, ptr::null_mut())
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
        recv(s, size, flags).and_then(|bytes| {
            crate::BytesOwner::try_new(bytes)
                .map(crate::BytesOwner::into_raw)
                .map_err(|_| Error::memory_error())
        })
    })();
    finish(result, ptr::null())
}

/// Allocate initialized syscall output without touching every page of a large receive.
fn zeroed_receive_buffer(size: usize) -> Result<Vec<u8>> {
    if size == 0 {
        return Ok(Vec::new());
    }
    let layout = std::alloc::Layout::array::<u8>(size).map_err(|_| Error::allocation())?;
    // SAFETY: size is nonzero and layout describes exactly size bytes.
    let data = unsafe { std::alloc::alloc_zeroed(layout) };
    if data.is_null() {
        return Err(Error::allocation());
    }
    // SAFETY: this initialized allocation has the layout Vec<u8> uses for size bytes.
    Ok(unsafe { Vec::from_raw_parts(data, size, size) })
}

pub(crate) fn recv(s: &Object, size: usize, flags: i32) -> Result<Vec<u8>> {
    let mut bytes = Vec::<u8>::new();
    if size != 0 {
        if flags & libc::MSG_TRUNC != 0 {
            // Linux TCP can report discarded bytes without touching the buffer.
            bytes = zeroed_receive_buffer(size)?;
        } else {
            bytes
                .try_reserve_exact(size)
                .map_err(|_| Error::allocation())?;
        }
        // SAFETY: the allocation has at least size writable bytes.
        let received =
            unsafe { receive(s, bytes.as_mut_ptr(), size, flags) }.map_err(errno_error)?;
        let n = received.min(size);
        // SAFETY: the kernel initialized the returned prefix or MSG_TRUNC
        // selected initialized storage. A reported packet length can exceed size.
        unsafe {
            bytes.set_len(n);
        }
        if n < size / 2 {
            crate::compact_bytes(&mut bytes);
        }
    }
    Ok(bytes)
}

/// Receive one message and preserve the source address, including a missing stream peer.
pub(crate) fn recv_from(s: &Object, size: usize, flags: i32) -> Result<(Vec<u8>, SocketName)> {
    let mut bytes = zeroed_receive_buffer(size)?;
    // SAFETY: zero initializes the entire socket-address storage, including padding.
    let mut peer: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let raw = fd(s);
    let (count, len) = socket_call(raw, libc::POLLIN, deadline(s), || {
        let mut len = size_of_val(&peer) as libc::socklen_t;
        // SAFETY: bytes and peer remain writable up to their respective capacities.
        let count = unsafe {
            libc::recvfrom(
                raw,
                bytes.as_mut_ptr().cast(),
                size,
                flags,
                (&raw mut peer).cast(),
                &mut len,
            )
        };
        let count = usize::try_from(count).map_err(|_| io::Error::last_os_error())?;
        Ok((count, len))
    })
    .map_err(errno_error)?;
    // MSG_TRUNC may report the full datagram size even when fewer bytes were copied.
    if count > size {
        bytes
            .try_reserve(count - size)
            .map_err(|_| Error::allocation())?;
    }
    bytes.resize(count, 0);
    if count < size / 2 {
        crate::compact_bytes(&mut bytes);
    }
    Ok((bytes, unpack_socket_name(&peer, len)?))
}

pub(crate) fn recv_into(s: &Object, buffer: &mut [u8], flags: i32) -> Result<usize> {
    // SAFETY: buffer exposes this many exclusively writable bytes.
    unsafe { receive(s, buffer.as_mut_ptr(), buffer.len(), flags) }.map_err(errno_error)
}

/// # Safety
/// The caller retains exclusive access to len writable bytes at buffer.
unsafe fn receive(socket: &Object, buffer: *mut u8, len: usize, flags: i32) -> io::Result<usize> {
    if len == 0 {
        return Ok(0);
    }
    let fd = fd(socket);
    socket_call(fd, libc::POLLIN, deadline(socket), || {
        // SAFETY: the caller supplies len writable bytes. Flags can make the
        // returned count differ from the number of bytes actually copied.
        let n = unsafe { libc::recv(fd, buffer.cast(), len, flags) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    })
}

pub(crate) fn send(socket: &Object, bytes: &[u8], flags: i32) -> Result<usize> {
    send_until(fd(socket), bytes, flags, deadline(socket))
}

/// Send one message to a socket address using the socket's existing timeout policy.
pub(crate) fn send_to(
    s: &Object,
    bytes: &[u8],
    flags: i32,
    address: &SocketAddress,
) -> Result<usize> {
    #[cfg(not(target_vendor = "apple"))]
    let flags = flags | libc::MSG_NOSIGNAL;
    let raw = fd(s);
    let (addr, len) = address.raw_parts();
    socket_call(raw, libc::POLLOUT, deadline(s), || {
        // SAFETY: the source slice and address live through each synchronous sendto.
        let count =
            unsafe { libc::sendto(raw, bytes.as_ptr().cast(), bytes.len(), flags, addr, len) };
        usize::try_from(count).map_err(|_| io::Error::last_os_error())
    })
    .map_err(errno_error)
}

fn send_until(raw: RawFd, bytes: &[u8], flags: i32, until: Option<Deadline>) -> Result<usize> {
    #[cfg(not(target_vendor = "apple"))]
    let flags = flags | libc::MSG_NOSIGNAL;
    socket_call(raw, libc::POLLOUT, until, || {
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
            .and_then(|flags| send(s, data, flags))
            .map(|n| Int::from_usize(n).into_raw()),
        word(0),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_sendall(s: &Object, data: &Bytes, flags: usize) {
    finish(c_int(flags).and_then(|flag| send_all(s, data, flag)), ());
}

pub(crate) fn send_all(s: &Object, data: &[u8], flags: i32) -> Result<()> {
    let raw = fd(s);
    let until = deadline(s);
    let mut remaining = data;
    // Empty input still makes one send: EBADF/EPIPE are observable.
    loop {
        let written = send_until(raw, remaining, flags, until)?;
        remaining = &remaining[written..];
        if remaining.is_empty() {
            return Ok(());
        }
        if until.is_some_and(|end| end.remaining().is_zero()) {
            return Err(errno_error(timed_out()));
        }
        if written == 0 {
            return Err(errno_error(io::Error::from_raw_os_error(libc::EPIPE)));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_fileno(s: &Object) -> usize {
    word(fd(s))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_close(s: &Object) {
    finish(close(s), ());
}

pub(crate) fn close(s: &Object) -> Result<()> {
    state(s, Socket::close).map_err(errno_error)
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
    finish(set_blocking_mode(s, flag != word(0)), ());
}

pub(crate) fn set_blocking_mode(s: &Object, blocking: bool) -> Result<()> {
    set_timeout(s, (!blocking).then_some(Duration::ZERO))
}

pub(crate) fn get_timeout(s: &Object) -> Option<Duration> {
    u64::try_from(state(s, |s| s.timeout_ns))
        .ok()
        .map(Duration::from_nanos)
}

pub(crate) fn set_timeout(s: &Object, timeout: Option<Duration>) -> Result<()> {
    let timeout_ns = match timeout {
        None => -1,
        Some(duration) => i64::try_from(duration.as_nanos()).map_err(|_| {
            Error::new(
                Kind::OverflowError,
                "timestamp too large to convert to C PyTime_t",
            )
        })?,
    };
    // CPython updates timeout even when fcntl subsequently reports EBADF.
    state(s, |s| s.timeout_ns = timeout_ns);
    set_blocking(fd(s), timeout.is_none()).map_err(errno_error)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getblocking(s: &Object) -> bool {
    get_timeout(s) != Some(Duration::ZERO)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_dup(s: &Object) -> *const Object {
    let result = (|| {
        let (raw, family, kind, proto) = state(s, |s| (s.fd, s.family, s.kind, s.proto));
        let timeout = get_timeout(s);
        // SAFETY: the kernel validates even a closed fd; its copy is non-inheritable from creation.
        let copy =
            checked(unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) }).map_err(errno_error)?;
        let child = handle(copy, timeout.is_none(), family, kind, proto)?;
        // socket.dup reapplies its source's blocking mode to the shared open file.
        set_timeout(&child, timeout)?;
        Ok(crate::ObjectOwner::into_raw(child))
    })();
    finish(result, ptr::null())
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
            set_option(s, level, name, value)
        })(),
        (),
    );
}

pub(crate) fn set_option(s: &Object, level: i32, name: i32, value: i32) -> Result<()> {
    setopt(fd(s), level, name, value).map_err(errno_error)
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

pub(crate) fn get_option_into(
    raw: RawFd,
    level: i32,
    name: i32,
    buffer: &mut [u8],
) -> Result<usize> {
    let mut len = libc::socklen_t::try_from(buffer.len())
        .map_err(|_| Error::new(Kind::OverflowError, "getsockopt buffer too large"))?;
    // SAFETY: the buffer is writable for the supplied capacity; the kernel returns its used length.
    checked(unsafe {
        libc::getsockopt(raw, level, name, buffer.as_mut_ptr().cast(), &raw mut len)
    })
    .map_err(errno_error)?;
    Ok(len as usize)
}

pub(crate) fn get_option(s: &Object, level: i32, name: i32) -> Result<i32> {
    int_option(fd(s), level, name)
}

fn int_option(raw: RawFd, level: i32, name: i32) -> Result<i32> {
    let mut bytes = [0; size_of::<i32>()];
    get_option_into(raw, level, name, &mut bytes)?;
    Ok(i32::from_ne_bytes(bytes))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_getsockopt(s: &Object, level: usize, name: usize) -> usize {
    finish(
        (|| get_option(s, c_int(level)?, c_int(name)?).map(word))(),
        word(0),
    )
}

#[cfg(test)]
mod tests {

    #[cfg(target_os = "linux")]
    #[test]
    fn truncated_datagram_uses_only_the_receive_buffer() {
        use std::os::fd::IntoRawFd;
        use std::os::unix::net::UnixDatagram;

        let (sender, socket) = UnixDatagram::pair().unwrap();
        let receiver = super::handle(
            socket.into_raw_fd(),
            true,
            libc::AF_UNIX,
            libc::SOCK_DGRAM,
            0,
        )
        .ok()
        .expect("socket handle");
        sender.send(b"larger packet").unwrap();
        let received = super::recv(&receiver, 3, libc::MSG_TRUNC)
            .ok()
            .expect("packet");
        assert_eq!(received.len(), 3);
        assert_eq!(received, b"lar");
    }

    use super::is_reverse_owner;

    #[cfg(target_os = "linux")]
    #[test]
    fn discarded_tcp_payload_returns_initialized_bytes() {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};
        use std::os::fd::IntoRawFd;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        let receiver = super::handle(
            socket.into_raw_fd(),
            true,
            libc::AF_INET,
            libc::SOCK_STREAM,
            0,
        )
        .ok()
        .expect("socket handle");
        sender.write_all(&[0x5a; 258]).unwrap();
        let scratch = vec![0xa5; 257];
        std::hint::black_box(&scratch);
        drop(scratch);
        let received = super::recv(&receiver, 257, libc::MSG_TRUNC | libc::MSG_WAITALL)
            .ok()
            .expect("discarded receive");
        assert_eq!(received.len(), 257);
        assert!(received.iter().all(|&byte| byte == 0));
        assert_eq!(
            super::recv(&receiver, 1, 0).ok().expect("trailing byte"),
            [0x5a]
        );
    }

    #[test]
    fn darwin_ptr_owner_leaves_real_host_aliases_available() {
        let v4 = "127.0.0.1".parse().unwrap();
        assert!(is_reverse_owner(v4, "1.0.0.127.in-addr.arpa"));
        assert!(is_reverse_owner(v4, "1.0.0.127.IN-ADDR.ARPA."));
        assert!(!is_reverse_owner(v4, "loopback.example"));
        assert!(!is_reverse_owner(v4, "2.0.0.127.in-addr.arpa"));
        let v6 = "2001:db8::1".parse().unwrap();
        assert!(is_reverse_owner(
            v6,
            "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        ));
        assert!(!is_reverse_owner(v6, "router.ip6.arpa"));
    }
}
