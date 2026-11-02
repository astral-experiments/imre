import _socket
import socket
from socket import NI_NUMERICHOST, NI_NUMERICSERV, getnameinfo


flags = NI_NUMERICHOST | NI_NUMERICSERV
assert getnameinfo is socket.getnameinfo is _socket.getnameinfo
assert socket.getnameinfo(("127.0.0.1", 53), flags) == ("127.0.0.1", "53")
for address, expected in [
    (("localhost", 0), socket.gaierror),
    (("127.0.0.1", 0, 0), OSError),
    (("::1", 0, 1 << 20), OverflowError),
]:
    try:
        socket.getnameinfo(address, flags)
    except expected:
        pass
    else:
        raise AssertionError(address)

if socket.has_ipv6:
    assert socket.getnameinfo(("::1", 53, 0, 0), flags) == ("::1", "53")
    for index, name in socket.if_nameindex():
        assert socket.if_nametoindex(name) == index
        assert socket.if_indextoname(index) == name
        # Link-local multicast addresses carry the zone in their numeric spelling.
        assert socket.getnameinfo(("ff02::1", 53, 0, index), flags) == ("ff02::1%" + name, "53")
