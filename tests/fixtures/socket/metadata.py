import _socket
import socket
from socket import AF_INET6, SOCK_DGRAM, SOCK_RAW, SOCK_RDM, SOCK_SEQPACKET, has_ipv6

assert socket.has_ipv6 is has_ipv6 is True
assert socket.AF_INET6 == AF_INET6
print(int(AF_INET6))

kinds = (SOCK_DGRAM, SOCK_RAW, SOCK_RDM, SOCK_SEQPACKET)
assert kinds == (socket.SOCK_DGRAM, socket.SOCK_RAW, socket.SOCK_RDM, socket.SOCK_SEQPACKET)
assert kinds == (_socket.SOCK_DGRAM, _socket.SOCK_RAW, _socket.SOCK_RDM, _socket.SOCK_SEQPACKET)
print(int(SOCK_DGRAM), int(SOCK_RAW), int(SOCK_RDM), int(SOCK_SEQPACKET))

for name in ("SOCK_DGRAM", "SOCK_RAW", "SOCK_RDM", "SOCK_SEQPACKET"):
    public = getattr(socket, name)
    native = getattr(_socket, name)
    try:
        setattr(socket, name, None)
        assert getattr(_socket, name) == native
        delattr(socket, name)
        assert not hasattr(socket, name)
        setattr(socket, name, public)
        setattr(_socket, name, None)
        assert getattr(socket, name) == public
        delattr(_socket, name)
        assert not hasattr(_socket, name)
        assert getattr(socket, name) == public
    finally:
        setattr(_socket, name, native)
        setattr(socket, name, public)
