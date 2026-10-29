//! One socket object owns a non-inheritable fd. All aliases observe explicit
//! close; dropping the last owner closes silently, including on error paths.
use super::exceptions::{Error, Kind, Result, finish};
use super::records::{RecordDescriptor, imre_record_new};
use super::{Bytes, Int, Object, Storage, StorageCell, Str, c_int, errno_error};
use num_traits::ToPrimitive;
use std::ffi::{CStr, CString};
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::ptr;

pub(super) struct Socket {
    fd: RawFd,
    blocking: bool,
    readers: usize,
    close_requested: bool,
}

impl Socket {
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
        let _ = self.close_fd();
    }
}

/// The raw input for socket.makefile borrows the same descriptor until its last reader closes.
pub(crate) struct SocketInput {
    socket: Option<crate::ObjectOwner>,
    buffer: io::Cursor<Vec<u8>>,
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
        })
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
            return unsafe { receive(self.as_raw_fd(), buffer.as_mut_ptr(), buffer.len(), 0) };
        }
        let count = io::Read::read(&mut io::BufRead::fill_buf(self)?, buffer)?;
        io::BufRead::consume(self, count);
        Ok(count)
    }
}

impl io::BufRead for SocketInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.buffer.position() >= self.buffer.get_ref().len() as u64 {
            let raw = self.as_raw_fd();
            let bytes = self.buffer.get_mut();
            bytes.clear();
            // SAFETY: try_new reserved this many bytes, and recv initializes the returned prefix.
            let count = unsafe { receive(raw, bytes.as_mut_ptr(), SOCKET_BUFFER_SIZE, 0)? };
            // SAFETY: receive initialized exactly count bytes in the reserved buffer.
            unsafe { bytes.set_len(count) };
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

pub(crate) fn fd(s: &Object) -> RawFd {
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

fn handle(raw: RawFd, blocking: bool) -> Result<crate::ObjectOwner> {
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
    crate::ObjectOwner::try_new(Object(StorageCell::new(Storage::Socket(Socket {
        fd: fd.into_raw_fd(),
        blocking,
        readers: 0,
        close_requested: false,
    }))))
    .map_err(|_| Error::memory_error())
}

pub(crate) fn open(family: i32, ty: i32, proto: i32) -> Result<crate::ObjectOwner> {
    // socket.socket uses -1 to request the default for each field.
    let family = if family == -1 { libc::AF_INET } else { family };
    let ty = if ty == -1 { libc::SOCK_STREAM } else { ty };
    let proto = if proto == -1 { 0 } else { proto };
    if family != libc::AF_INET || ty != libc::SOCK_STREAM {
        return Err(Error::new(
            Kind::NotImplementedError,
            "only AF_INET stream sockets are supported",
        ));
    }
    #[cfg(target_os = "linux")]
    let ty = ty | libc::SOCK_CLOEXEC;
    // SAFETY: socket takes only scalars, and handle acquires the fresh descriptor.
    let raw = checked(unsafe { libc::socket(family, ty, proto) }).map_err(errno_error)?;
    handle(raw, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_socket_new(family: usize, ty: usize, proto: usize) -> *const Object {
    finish(
        (|| open(c_int(family)?, c_int(ty)?, c_int(proto)?))().map(crate::ObjectOwner::into_raw),
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
    let rows = getaddrinfo(Some(host), None, libc::AF_INET, 0, 0, 0)?;
    Ok(endpoint(&rows[0].addr).0)
}

pub(crate) struct AddressInfo {
    pub(crate) ty: i32,
    pub(crate) protocol: i32,
    pub(crate) canonical: String,
    pub(crate) addr: libc::sockaddr_in,
}

struct AddrInfoList(*mut libc::addrinfo);

impl Drop for AddrInfoList {
    fn drop(&mut self) {
        // SAFETY: the list was returned by getaddrinfo and remains uniquely owned.
        unsafe { libc::freeaddrinfo(self.0) };
    }
}

pub(crate) fn getaddrinfo(
    host: Option<&str>,
    service: Option<&str>,
    family: i32,
    ty: i32,
    protocol: i32,
    flags: i32,
) -> Result<Vec<AddressInfo>> {
    let text = |value: &str| {
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
    if res.is_null() {
        return Err(Error::new(
            Kind::OSError,
            "getaddrinfo returned an empty list",
        ));
    }
    let list = AddrInfoList(res);
    let mut cursor = list.0;
    let mut rows = Vec::new();
    // SAFETY: all nodes and canonical names remain live until the list guard is dropped.
    while let Some(entry) = unsafe { cursor.as_ref() } {
        if entry.ai_family != libc::AF_INET {
            return Err(Error::new(
                Kind::NotImplementedError,
                "only IPv4 address results are supported",
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
            // SAFETY: AF_INET specifies the layout of this node's initialized address.
            addr: unsafe { *entry.ai_addr.cast::<libc::sockaddr_in>() },
        });
        cursor = entry.ai_next;
    }
    Ok(rows)
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

/// Reverse resolution must consider aliases before falling back to the primary name.
/// Copy both while the legacy resolver's hostent is protected.
pub(crate) fn fqdn(name: &str) -> Result<String> {
    use std::net::{IpAddr, ToSocketAddrs};
    use std::sync::Mutex;
    unsafe extern "C" {
        fn gethostbyaddr(
            addr: *const libc::c_void,
            len: libc::socklen_t,
            family: libc::c_int,
        ) -> *mut libc::hostent;
    }
    static REVERSE: Mutex<()> = Mutex::new(());
    let mut name = name.trim().to_owned();
    if name.is_empty() || name == "0.0.0.0" {
        let mut buffer = [0u8; 256];
        let n = buffer.len() - 1;
        // SAFETY: the buffer has n writable bytes and a trailing zero even if truncated.
        checked(unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), n) })
            .map_err(errno_error)?;
        name = CStr::from_bytes_until_nul(&buffer)
            .unwrap()
            .to_string_lossy()
            .into_owned();
    }
    if name.as_bytes().contains(&0) {
        return Err(Error::new(
            Kind::TypeError,
            "gethostbyaddr() argument 1 must be encoded string without null bytes",
        ));
    }
    let addr = name.parse::<IpAddr>().ok().or_else(|| {
        (name.as_str(), 0)
            .to_socket_addrs()
            .ok()?
            .next()
            .map(|address| address.ip())
    });
    let Some(addr) = addr else {
        return Ok(name);
    };
    let _guard = REVERSE.lock().unwrap_or_else(|error| error.into_inner());
    // SAFETY: the address bytes live through gethostbyaddr. Its terminated name
    // and aliases remain valid until another legacy lookup on this thread.
    unsafe {
        let host = match addr {
            IpAddr::V4(ip) => gethostbyaddr(ip.octets().as_ptr().cast(), 4, libc::AF_INET),
            IpAddr::V6(ip) => gethostbyaddr(ip.octets().as_ptr().cast(), 16, libc::AF_INET6),
        };
        if host.is_null() || (*host).h_name.is_null() {
            return Ok(name);
        }
        let primary = CStr::from_ptr((*host).h_name)
            .to_string_lossy()
            .into_owned();
        if primary.contains('.') {
            return Ok(primary);
        }
        let mut aliases = (*host).h_aliases;
        while !aliases.is_null() {
            // CPython's gethost_common also accepts unaligned hostent pointer lists.
            let entry = aliases.read_unaligned();
            if entry.is_null() {
                break;
            }
            aliases = aliases.add(1);
            let alias = CStr::from_ptr(entry).to_string_lossy();
            if alias.contains('.') {
                #[cfg(target_vendor = "apple")]
                if is_reverse_owner(addr, &alias) {
                    continue;
                }
                return Ok(alias.into_owned());
            }
        }
        Ok(primary)
    }
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

pub(crate) fn address(host: &str, port: i64, name: &str) -> Result<libc::sockaddr_in> {
    let ip = resolve(host)?;
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
        address(&**pair.cast::<*const Str>(), i64::from(port), name)
    }
}

pub(crate) fn bind(s: &Object, addr: &libc::sockaddr_in) -> Result<()> {
    // SAFETY: bind reads the initialized stack address during this call.
    checked(unsafe {
        libc::bind(
            fd(s),
            (addr as *const libc::sockaddr_in).cast(),
            size_of_val(addr) as _,
        )
    })
    .map(|_| ())
    .map_err(errno_error)
}

/// # Safety
/// The operand is a rooted exact (str, int) address tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_bind(s: &Object, pair: *const usize) {
    // SAFETY: unpack borrows the declared fields.
    finish(
        unsafe { unpack(pair, "bind") }.and_then(|addr| bind(s, &addr)),
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
        unsafe { unpack(pair, "connect") }.and_then(|addr| connect_address(s, &addr)),
        (),
    );
}

pub(crate) fn connect_address(s: &Object, addr: &libc::sockaddr_in) -> Result<()> {
    connect(fd(s), addr, state(s, |s| s.blocking)).map_err(errno_error)
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
) -> io::Result<(i32, libc::sockaddr_in)> {
    // SAFETY: all fields have a valid zero representation before the kernel writes them.
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut len = size_of_val(&addr) as libc::socklen_t;
    let ret = checked(call((&raw mut addr).cast(), &raw mut len))?;
    Ok((ret, addr))
}

pub(crate) fn sockname(s: &Object) -> Result<libc::sockaddr_in> {
    // SAFETY: peer_call supplies a writable sockaddr and resets its capacity.
    peer_call(|a, n| unsafe { libc::getsockname(fd(s), a, n) })
        .map(|(_, a)| a)
        .map_err(errno_error)
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
        sockname(s).map(|addr| unsafe { addr_tuple(layout, addr) }),
        ptr::null_mut(),
    )
}

pub(crate) fn accept(s: &Object) -> Result<(crate::ObjectOwner, libc::sockaddr_in)> {
    let raw = fd(s);
    let (child, addr) = retry(|| {
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
    Ok((handle(child, true)?, addr))
}

/// # Safety
/// Static descriptors describe (socket, (str, int)) and (str, int), respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_accept(
    s: &Object,
    layout: *const RecordDescriptor,
    addr_layout: *const RecordDescriptor,
) -> *mut usize {
    let result = accept(s).map(|(child, a)| {
        // SAFETY: both descriptors match these transferred owners. No fallible syscall remains.
        unsafe {
            let pair = imre_record_new(layout);
            pair.cast::<*const Object>()
                .write(crate::ObjectOwner::into_raw(child));
            pair.add(1)
                .cast::<*mut usize>()
                .write(addr_tuple(addr_layout, a));
            pair
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

pub(crate) fn recv(s: &Object, size: usize, flags: i32) -> Result<Vec<u8>> {
    let mut bytes = Vec::<u8>::new();
    if size != 0 {
        bytes
            .try_reserve_exact(size)
            .map_err(|_| Error::allocation())?;
        // SAFETY: the empty Vec owns size writable bytes in its reserved allocation.
        let received =
            unsafe { receive(fd(s), bytes.as_mut_ptr(), size, flags) }.map_err(errno_error)?;
        let n = received.min(size);
        // SAFETY: even when recv reports the full packet length, the prefix
        // retained here stays within the receive allocation.
        unsafe {
            bytes.set_len(n);
        }
        if n < size / 2 {
            bytes.shrink_to_fit();
        }
    }
    Ok(bytes)
}

pub(crate) fn recv_into(s: &Object, buffer: &mut [u8], flags: i32) -> Result<usize> {
    // SAFETY: buffer exposes this many exclusively writable bytes.
    unsafe { receive(fd(s), buffer.as_mut_ptr(), buffer.len(), flags) }.map_err(errno_error)
}

/// # Safety
/// The caller retains exclusive access to len writable bytes at buffer.
unsafe fn receive(fd: RawFd, buffer: *mut u8, len: usize, flags: i32) -> io::Result<usize> {
    if len == 0 {
        return Ok(0);
    }
    retry(|| {
        // SAFETY: the caller supplies len writable bytes. Flags can make the
        // returned count differ from the number of bytes actually copied.
        let n = unsafe { libc::recv(fd, buffer.cast(), len, flags) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    })
}

pub(crate) fn send(raw: RawFd, bytes: &[u8], flags: i32) -> Result<usize> {
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
    finish(c_int(flags).and_then(|flag| send_all(s, data, flag)), ());
}

pub(crate) fn send_all(s: &Object, data: &[u8], flags: i32) -> Result<()> {
    let raw = fd(s);
    let mut remaining = data;
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
    // CPython updates its socket timeout state even if fcntl later reports EBADF.
    state(s, |s| s.blocking = blocking);
    set_blocking(fd(s), blocking).map_err(errno_error)
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
        let copy =
            checked(unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) }).map_err(errno_error)?;
        let child = handle(copy, blocking)?;
        // socket.dup reapplies its source's blocking mode to the shared open file.
        set_blocking(copy, blocking).map_err(errno_error)?;
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

#[cfg(test)]
mod tests {

    #[cfg(target_os = "linux")]
    #[test]
    fn truncated_datagram_uses_only_the_receive_buffer() {
        use std::os::fd::IntoRawFd;
        use std::os::unix::net::UnixDatagram;

        let (sender, socket) = UnixDatagram::pair().unwrap();
        let receiver = super::handle(socket.into_raw_fd(), true)
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
