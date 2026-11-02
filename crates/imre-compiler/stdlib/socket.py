"""Socket helpers using the runtime's native socket operations."""

import _socket
from _socket import SOCK_DGRAM, SOCK_RAW, SOCK_RDM, SOCK_SEQPACKET
from _socket import SOCK_STREAM, close, error, has_ipv6, socket

if hasattr(_socket, "SocketType"):
    SocketType = _socket.SocketType
if hasattr(_socket, "AF_INET6"):
    AF_INET6 = _socket.AF_INET6
if hasattr(_socket, "IPPROTO_UDP"):
    IPPROTO_UDP = _socket.IPPROTO_UDP
if hasattr(_socket, "IPPROTO_IPV6"):
    IPPROTO_IPV6 = _socket.IPPROTO_IPV6
if hasattr(_socket, "IPV6_V6ONLY"):
    IPV6_V6ONLY = _socket.IPV6_V6ONLY


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
