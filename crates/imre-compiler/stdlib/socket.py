"""Socket helpers using the runtime's native socket operations."""

import _socket
from _socket import close, error, has_ipv6, socket

for _name in ("NI_DGRAM", "NI_NAMEREQD", "NI_NOFQDN", "NI_NUMERICHOST", "NI_NUMERICSERV",
              "gaierror", "getnameinfo", "if_indextoname", "if_nameindex", "if_nametoindex"):
    if hasattr(_socket, _name):
        globals()[_name] = getattr(_socket, _name)
del _name

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
