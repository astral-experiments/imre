"""Socket helpers using the runtime's native socket operations."""

import _socket
import errno
import io
import os
from _socket import close, error, has_ipv6, socket

_GLOBAL_DEFAULT_TIMEOUT = object()

for _name in ("NI_DGRAM", "NI_NAMEREQD", "NI_NOFQDN", "NI_NUMERICHOST", "NI_NUMERICSERV",
              "gaierror", "getnameinfo", "getservbyname", "getservbyport",
              "if_indextoname", "if_nameindex", "if_nametoindex"):
    if hasattr(_socket, _name):
        globals()[_name] = getattr(_socket, _name)
del _name

if hasattr(_socket, "gethostname"):
    gethostname = _socket.gethostname
if hasattr(_socket, "gethostbyname"):
    gethostbyname = _socket.gethostbyname
if hasattr(_socket, "gethostbyaddr"):
    gethostbyaddr = _socket.gethostbyaddr
if hasattr(_socket, "herror"):
    herror = _socket.herror
if hasattr(_socket, "SocketType"):
    SocketType = _socket.SocketType
if hasattr(_socket, "IPPROTO_UDP"):
    IPPROTO_UDP = _socket.IPPROTO_UDP
if hasattr(_socket, "IPPROTO_IPV6"):
    IPPROTO_IPV6 = _socket.IPPROTO_IPV6
if hasattr(_socket, "IPV6_V6ONLY"):
    IPV6_V6ONLY = _socket.IPV6_V6ONLY


_blocking_errnos = {errno.EAGAIN, errno.EWOULDBLOCK}


class SocketIO(io.RawIOBase):

    """Raw I/O implementation for stream sockets.

    This class supports the makefile() method on sockets.  It provides
    the raw I/O interface on top of a socket object.
    """

    def __init__(self, sock, mode):
        if mode not in ("r", "w", "rw", "rb", "wb", "rwb"):
            raise ValueError("invalid mode: %r" % mode)
        self._sock = sock
        if "b" not in mode:
            mode += "b"
        self._mode = mode
        self._reading = "r" in mode
        self._writing = "w" in mode
        self._timeout_occurred = False

    def _checkClosed(self):
        if self.closed:
            raise ValueError("I/O operation on closed file.")

    def _checkReadable(self):
        if self.readable() is not True:
            raise io.UnsupportedOperation("File or stream is not readable.")

    def _checkWritable(self):
        if self.writable() is not True:
            raise io.UnsupportedOperation("File or stream is not writable.")

    def readinto(self, b):
        """Read up to len(b) bytes into the writable buffer *b* and return
        the number of bytes read.  If the socket is non-blocking and no bytes
        are available, None is returned.

        If *b* is non-empty, a 0 return value indicates that the connection
        was shutdown at the other end.
        """
        self._checkClosed()
        self._checkReadable()
        if self._timeout_occurred:
            raise OSError("cannot read from timed out object")
        try:
            return self._sock.recv_into(b)
        except timeout:
            self._timeout_occurred = True
            raise
        except error as e:
            if e.errno in _blocking_errnos:
                return None
            raise

    def write(self, b):
        """Write the given bytes or bytearray object *b* to the socket
        and return the number of bytes written.  This can be less than
        len(b) if not all data could be written.  If the socket is
        non-blocking and no bytes could be written None is returned.
        """
        self._checkClosed()
        self._checkWritable()
        try:
            return self._sock.send(b)
        except error as e:
            if e.errno in _blocking_errnos:
                return None
            raise

    def readable(self):
        """True if the SocketIO is open for reading.
        """
        if self.closed:
            raise ValueError("I/O operation on closed socket.")
        return self._reading

    def writable(self):
        """True if the SocketIO is open for writing.
        """
        if self.closed:
            raise ValueError("I/O operation on closed socket.")
        return self._writing

    def seekable(self):
        """True if the SocketIO is open for seeking.
        """
        if self.closed:
            raise ValueError("I/O operation on closed socket.")
        return super().seekable()

    def fileno(self):
        """Return the file descriptor of the underlying socket.
        """
        self._checkClosed()
        return self._sock.fileno()

    @property
    def name(self):
        if not self.closed:
            return self.fileno()
        else:
            return -1

    @property
    def mode(self):
        return self._mode

    def close(self):
        """Close the SocketIO object.  This doesn't close the underlying
        socket, except if all references to it have disappeared.
        """
        if self.closed:
            return
        io.RawIOBase.close(self)
        self._sock._decref_socketios()
        self._sock = None


def _intenum_converter(value, enum_klass):
    try:
        return enum_klass(value)
    except ValueError:
        return value


def getaddrinfo(host, port, family=0, type=0, proto=0, flags=0):
    return [(_intenum_converter(af, AddressFamily), _intenum_converter(kind, SocketKind),
             protocol, canonname, sockaddr)
            for af, kind, protocol, canonname, sockaddr
            in _socket.getaddrinfo(host, port, family, type, proto, flags)]


if hasattr(_socket, "socketpair"):
    def socketpair(family=None, type=SOCK_STREAM, proto=0):
        """Create a pair of connected sockets."""
        if family is None:
            try:
                family = AF_UNIX
            except NameError:
                family = AF_INET
        return _socket.socketpair(family, type, proto)


def create_server(address, *, family=AF_INET, backlog=None, reuse_port=False,
                  dualstack_ipv6=False):
    """Create a listening stream socket bound to address."""
    if reuse_port and not hasattr(_socket, "SO_REUSEPORT"):
        raise ValueError("SO_REUSEPORT not supported on this platform")
    if dualstack_ipv6:
        if not has_dualstack_ipv6():
            raise ValueError("dualstack_ipv6 not supported on this platform")
        if family != AF_INET6:
            raise ValueError("dualstack_ipv6 requires AF_INET6 family")
    sock = socket(family, SOCK_STREAM)
    try:
        if os.name not in ('nt', 'cygwin') and \
                hasattr(_socket, 'SO_REUSEADDR'):
            try:
                sock.setsockopt(SOL_SOCKET, SO_REUSEADDR, 1)
            except error:
                pass
        if reuse_port and family in (AF_INET, AF_INET6):
            sock.setsockopt(SOL_SOCKET, SO_REUSEPORT, 1)
        if has_ipv6 and family == AF_INET6:
            if dualstack_ipv6:
                sock.setsockopt(IPPROTO_IPV6, IPV6_V6ONLY, 0)
            elif hasattr(_socket, "IPV6_V6ONLY") and \
                    hasattr(_socket, "IPPROTO_IPV6"):
                sock.setsockopt(IPPROTO_IPV6, IPV6_V6ONLY, 1)
        try:
            sock.bind(address)
        except error as err:
            msg = '%s (while attempting to bind on address %r)' % \
                (err.strerror, address)
            raise error(err.errno, msg) from None
        if backlog is None:
            sock.listen()
        else:
            sock.listen(backlog)
        return sock
    except error:
        sock.close()
        raise


def create_connection(address, timeout=_GLOBAL_DEFAULT_TIMEOUT,
                      source_address=None, *, all_errors=False):
    """Connect to *address* and return the socket object.

    Convenience function.  Connect to *address* (a 2-tuple ``(host,
    port)``) and return the socket object.  Passing the optional
    *timeout* parameter will set the timeout on the socket instance
    before attempting to connect.  If no *timeout* is supplied, the
    global default timeout setting returned by :func:`getdefaulttimeout`
    is used.  If *source_address* is set it must be a tuple of (host, port)
    for the socket to bind as a source address before making the connection.
    A host of '' or port 0 tells the OS to use the default. When a connection
    cannot be created, raises the last error if *all_errors* is False,
    and an ExceptionGroup of all errors if *all_errors* is True.
    """

    host, port = address
    exceptions = []
    for res in getaddrinfo(host, port, 0, SOCK_STREAM):
        af, socktype, proto, canonname, sa = res
        sock = None
        try:
            sock = socket(af, socktype, proto)
            if timeout is not _GLOBAL_DEFAULT_TIMEOUT:
                sock.settimeout(timeout)
            if source_address:
                sock.bind(source_address)
            sock.connect(sa)
            # Break explicitly a reference cycle
            exceptions.clear()
            return sock

        except error as exc:
            if not all_errors:
                exceptions.clear()  # raise only the last error
            exceptions.append(exc)
            if sock is not None:
                sock.close()

    if len(exceptions):
        try:
            if not all_errors:
                raise exceptions[0]
            raise ExceptionGroup("create_connection failed", exceptions)
        finally:
            # Break explicitly a reference cycle
            exceptions.clear()
    else:
        raise error("getaddrinfo returns an empty list")


def has_dualstack_ipv6():
    """Return True if the platform supports creating a SOCK_STREAM socket
    which can handle both AF_INET and AF_INET6 (IPv4 / IPv6) connections.
    """
    if not has_ipv6 \
            or not hasattr(_socket, 'IPPROTO_IPV6') \
            or not hasattr(_socket, 'IPV6_V6ONLY'):
        return False
    try:
        with socket(AF_INET6, SOCK_STREAM) as sock:
            sock.setsockopt(IPPROTO_IPV6, IPV6_V6ONLY, 0)
            return True
    except error:
        return False
