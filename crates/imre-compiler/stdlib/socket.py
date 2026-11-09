"""Socket helpers using the runtime's native socket operations."""

import _socket
import errno
import io
import os
from _socket import close, error, has_ipv6

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


class _GiveupOnSendfile(Exception):
    """Retry with the file reader and the socket send method."""


class socket(_socket.socket):

    """A subclass of _socket.socket adding the makefile() method."""

    __slots__ = ["__weakref__", "_io_refs", "_closed"]

    def __init__(self, family=-1, type=-1, proto=-1, fileno=None):
        # For user code address family and type values are IntEnum members, but
        # for the underlying _socket.socket they're just integers. The
        # constructor of _socket.socket converts the given argument to an
        # integer automatically.
        if fileno is None:
            if family == -1:
                family = AF_INET
            if type == -1:
                type = SOCK_STREAM
            if proto == -1:
                proto = 0
        _socket.socket.__init__(self, family, type, proto, fileno)
        self._io_refs = 0
        self._closed = False

    def __enter__(self):
        return self

    def __exit__(self, *args):
        if not self._closed:
            self.close()

    def __repr__(self):
        """Wrap __repr__() to reveal the real class name and socket
        address(es).
        """
        closed = getattr(self, '_closed', False)
        s = "<%s.%s%s fd=%i, family=%s, type=%s, proto=%i" \
            % (self.__class__.__module__,
               self.__class__.__qualname__,
               " [closed]" if closed else "",
               self.fileno(),
               self.family,
               self.type,
               self.proto)
        if not closed:
            # getsockname and getpeername may not be available on WASI.
            try:
                laddr = self.getsockname()
                if laddr:
                    s += ", laddr=%s" % str(laddr)
            except (error, AttributeError):
                pass
            try:
                raddr = self.getpeername()
                if raddr:
                    s += ", raddr=%s" % str(raddr)
            except (error, AttributeError):
                pass
        s += '>'
        return s

    def __getstate__(self):
        raise TypeError(f"cannot pickle {self.__class__.__name__!r} object")

    def dup(self):
        """dup() -> socket object

        Duplicate the socket. Return a new socket object connected to the same
        system resource. The new socket is non-inheritable.
        """
        fd = os.dup(self.fileno())
        sock = self.__class__(self.family, self.type, self.proto, fileno=fd)
        sock.settimeout(self.gettimeout())
        return sock

    def accept(self):
        """accept() -> (socket object, address info)

        Wait for an incoming connection.  Return a new socket
        representing the connection, and the address of the client.
        For IP sockets, the address info is a pair (hostaddr, port).
        """
        fd, addr = self._accept()
        sock = socket(self.family, self.type, self.proto, fileno=fd)
        # Issue #7995: if no default timeout is set and the listening
        # socket had a (non-zero) timeout, force the new socket in blocking
        # mode to override platform-specific socket flags inheritance.
        if getdefaulttimeout() is None and self.gettimeout():
            sock.setblocking(True)
        return sock, addr

    def makefile(self, mode="r", buffering=None, *,
                 encoding=None, errors=None, newline=None):
        """makefile(...) -> an I/O stream connected to the socket

        The arguments are as for io.open() after the filename, except the only
        supported mode values are 'r' (default), 'w', 'b', or a combination of
        those.
        """
        # XXX refactor to share code?
        if not set(mode) <= {"r", "w", "b"}:
            raise ValueError("invalid mode %r (only r, w, b allowed)" % (mode,))
        writing = "w" in mode
        reading = "r" in mode or not writing
        assert reading or writing
        binary = "b" in mode
        rawmode = ""
        if reading:
            rawmode += "r"
        if writing:
            rawmode += "w"
        raw = SocketIO(self, rawmode)
        self._io_refs += 1
        if buffering is None:
            buffering = -1
        if buffering < 0:
            buffering = io.DEFAULT_BUFFER_SIZE
        if buffering == 0:
            if not binary:
                raise ValueError("unbuffered streams must be binary")
            return raw
        if reading and writing:
            buffer = io.BufferedRWPair(raw, raw, buffering)
        elif reading:
            buffer = io.BufferedReader(raw, buffering)
        else:
            assert writing
            buffer = io.BufferedWriter(raw, buffering)
        if binary:
            return buffer
        encoding = io.text_encoding(encoding)
        text = io.TextIOWrapper(buffer, encoding, errors, newline)
        text.mode = mode
        return text

    def _sendfile_kernel(self, target, file, offset, count):
        # Lazy import to improve module import time
        import selectors

        try:
            fileno = file.fileno()
        except (AttributeError, io.UnsupportedOperation) as err:
            raise _GiveupOnSendfile(err)  # not a regular file
        try:
            fsize = os.fstat(fileno).st_size
        except OSError as err:
            raise _GiveupOnSendfile(err)  # not a regular file
        if not fsize:
            return 0  # empty file
        # Truncate to 1GiB to avoid OverflowError, see bpo-38319.
        blocksize = min(count or fsize, 2 ** 30)
        timeout = self.gettimeout()
        if timeout == 0:
            raise ValueError("non-blocking sockets are not supported")
        # poll/select have the advantage of not requiring any
        # extra file descriptor, contrarily to epoll/kqueue
        # (also, they require a single syscall).
        if hasattr(selectors, 'PollSelector'):
            selector = selectors.PollSelector()
        else:
            selector = selectors.SelectSelector()
        if target is not None:
            _socket.socket._check_sendfile_owner(self, target)
        selector.register(_socket.socket.fileno(self), selectors.EVENT_WRITE)

        total_sent = 0
        # localize variable access to minimize overhead
        selector_select = selector.select
        try:
            while True:
                if timeout and not selector_select(timeout):
                    _socket.socket._check_sendfile_owner(self, target)
                    raise TimeoutError('timed out')
                if count:
                    blocksize = min(count - total_sent, blocksize)
                    if blocksize <= 0:
                        break
                try:
                    sent = _socket.socket._sendfile_block(self, target, fileno, offset, blocksize)
                except BlockingIOError:
                    if not timeout:
                        # Block until the socket is ready to send some
                        # data; avoids hogging CPU resources.
                        selector_select()
                    continue
                except OSError as err:
                    if total_sent == 0:
                        # We can get here for different reasons, the main
                        # one being 'file' is not a regular mmap(2)-like
                        # file, in which case we'll fall back on using
                        # plain send().
                        raise _GiveupOnSendfile(err)
                    raise err from None
                else:
                    if sent == 0:
                        break  # EOF
                    offset += sent
                    total_sent += sent
            return total_sent
        finally:
            if total_sent > 0 and hasattr(file, 'seek'):
                file.seek(offset)
    def _sendfile_buffered(self, target, file, offset, count, wrap_send):
        if self.gettimeout() == 0:
            raise ValueError("non-blocking sockets are not supported")
        if offset:
            file.seek(offset)
        blocksize = min(count, 8192) if count else 8192
        total_sent = 0
        # localize variable access to minimize overhead
        file_read = file.read
        sock_send = self.send
        session = None
        if wrap_send is not None:
            sock_send, session = wrap_send(sock_send)
        try:
            while True:
                if count:
                    blocksize = min(count - total_sent, blocksize)
                    if blocksize <= 0:
                        break
                data = memoryview(file_read(blocksize))
                if not data:
                    break  # EOF
                while True:
                    try:
                        sent = _socket.socket._sendfile_write(self, target, sock_send, data, session)
                    except BlockingIOError:
                        continue
                    else:
                        total_sent += sent
                        if sent < len(data):
                            data = data[sent:]
                        else:
                            break
            return total_sent
        finally:
            if total_sent > 0 and hasattr(file, 'seek'):
                file.seek(offset + total_sent)

    def _check_sendfile_params(self, file, offset, count):
        if 'b' not in getattr(file, 'mode', 'b'):
            raise ValueError("file should be opened in binary mode")
        if not self.type & SOCK_STREAM:
            raise ValueError("only SOCK_STREAM type sockets are supported")
        if count is not None:
            if not isinstance(count, int):
                raise TypeError(
                    "count must be a positive integer (got {!r})".format(count))
            if count <= 0:
                raise ValueError(
                    "count must be a positive integer (got {!r})".format(count))

    def _sendfile_transfer(self, file, offset, count, *, kernel, fallback, wrap_send=None):
        target = _socket.socket._sendfile_owner(self)
        self._check_sendfile_params(file, offset, count)
        if kernel:
            try:
                if not hasattr(os, 'sendfile'):
                    raise _GiveupOnSendfile("os.sendfile() not available on this platform")
                return self._sendfile_kernel(target, file, offset, count)
            except _GiveupOnSendfile:
                if not fallback:
                    raise
        return self._sendfile_buffered(target, file, offset, count, wrap_send)

    def _sendfile_use_sendfile(self, file, offset=0, count=None):
        return self._sendfile_transfer(file, offset, count, kernel=True, fallback=False)

    def _sendfile_use_send(self, file, offset=0, count=None):
        return self._sendfile_transfer(file, offset, count, kernel=False, fallback=True)

    def sendfile(self, file, offset=0, count=None):
        """sendfile(file[, offset[, count]]) -> sent

        Send a file until EOF is reached by using high-performance
        os.sendfile() and return the total number of bytes which
        were sent.
        *file* must be a regular file object opened in binary mode.
        If os.sendfile() is not available (e.g. Windows) or file is
        not a regular file socket.send() will be used instead.
        *offset* tells from where to start reading the file.
        If specified, *count* is the total number of bytes to transmit
        as opposed to sending the file until EOF is reached.
        File position is updated on return or also in case of error in
        which case file.tell() can be used to figure out the number of
        bytes which were sent.
        The socket must be of SOCK_STREAM type.
        Non-blocking sockets are not supported.
        """
        return self._sendfile_transfer(file, offset, count, kernel=True, fallback=True)

    def _decref_socketios(self):
        if self._io_refs > 0:
            self._io_refs -= 1
        if self._closed:
            self.close()

    def _real_close(self, _ss=_socket.socket):
        # This function should not reference any globals. See issue #808164.
        _ss.close(self)

    def close(self):
        # This function should not reference any globals. See issue #808164.
        self._closed = True
        if self._io_refs <= 0:
            self._real_close()

    def detach(self):
        """detach() -> file descriptor

        Close the socket object without closing the underlying file descriptor.
        The object cannot be used after this call, but the file descriptor
        can be reused for other purposes.  The file descriptor is returned.
        """
        self._closed = True
        return super().detach()

    @property
    def family(self):
        """Read-only access to the address family for this socket.
        """
        return _intenum_converter(super().family, AddressFamily)

    @property
    def type(self):
        """Read-only access to the socket type.
        """
        return _intenum_converter(super().type, SocketKind)

    def get_inheritable(self):
        return os.get_inheritable(self.fileno())

    def set_inheritable(self, inheritable):
        os.set_inheritable(self.fileno(), inheritable)


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


if os.name == 'posix':
    def fromfd(fd, family, type, proto=0):
        nfd = os.dup(fd)
        return socket(family, type, proto, nfd)


if hasattr(_socket.socket, "sendmsg"):
    def send_fds(sock, buffers, fds, flags=0, address=None):
        """ send_fds(sock, buffers, fds[, flags[, address]]) -> integer

        Send the list of file descriptors fds over an AF_UNIX socket.
        """
        import array

        return sock.sendmsg(buffers, [(_socket.SOL_SOCKET,
            _socket.SCM_RIGHTS, array.array("i", fds))])

if hasattr(_socket.socket, "recvmsg"):
    def recv_fds(sock, bufsize, maxfds, flags=0):
        """ recv_fds(sock, bufsize, maxfds[, flags]) -> (data, list of file
        descriptors, msg_flags, address)

        Receive up to maxfds file descriptors returning the message
        data and a list containing the descriptors.
        """
        import array

        # Array of ints
        fds = array.array("i")
        msg, ancdata, flags, addr = sock.recvmsg(bufsize,
            _socket.CMSG_LEN(maxfds * fds.itemsize))
        for cmsg_level, cmsg_type, cmsg_data in ancdata:
            if (cmsg_level == _socket.SOL_SOCKET and cmsg_type == _socket.SCM_RIGHTS):
                fds.frombytes(cmsg_data[:
                        len(cmsg_data) - (len(cmsg_data) % fds.itemsize)])

        return msg, list(fds), flags, addr


if hasattr(_socket, "socketpair"):
    def socketpair(family=None, type=SOCK_STREAM, proto=0):
        """Create a pair of connected sockets."""
        if family is None:
            try:
                family = AF_UNIX
            except NameError:
                family = AF_INET
        a, b = _socket.socketpair(family, type, proto)
        a = socket(family, type, proto, a.detach())
        b = socket(family, type, proto, b.detach())
        return a, b


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
