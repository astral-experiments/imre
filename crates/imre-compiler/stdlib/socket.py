"""Socket helpers using the runtime's native socket operations."""

import _socket
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
