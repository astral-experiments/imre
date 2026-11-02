import _socket
from _socket import getaddrinfo as raw_getaddrinfo, getservbyport as raw_getservbyport
import socket
from socket import AddressFamily, SocketKind, getaddrinfo, getservbyname

assert socket.gaierror is _socket.gaierror
assert issubclass(socket.gaierror, OSError)
assert (socket.gaierror.__module__, socket.gaierror.__qualname__, socket.gaierror.__base__) == ("socket", "gaierror", OSError)
assert socket.AI_NUMERICSERV is socket.AddressInfo.AI_NUMERICSERV
try:
    socket.getaddrinfo("127.0.0.1", "http", flags=socket.AI_NUMERICSERV)
except socket.gaierror as exc:
    assert exc.errno in (socket.EAI_NONAME, socket.EAI_NODATA, socket.EAI_SERVICE)
else:
    raise AssertionError("AI_NUMERICSERV accepted a named service")
assert list(AddressFamily) == sorted(AddressFamily, key=int)
assert list(SocketKind) == sorted(SocketKind, key=int)
arguments = ("127.0.0.1", 80, socket.AF_INET, socket.SOCK_STREAM)
(public,) = getaddrinfo(*arguments)
(raw,) = raw_getaddrinfo(*arguments)
assert raw_getaddrinfo(b"127.0.0.1\0ignored", b"80\0ignored", socket.AF_INET, socket.SOCK_STREAM) == [raw]
assert public == raw
assert public[0] is AddressFamily.AF_INET is socket.AF_INET
assert public[1] is SocketKind.SOCK_STREAM is socket.SOCK_STREAM
assert type(raw[0]) is type(raw[1]) is int
assert (public[3], public[4]) == ("", ("127.0.0.1", 80))
try:
    socket.getaddrinfo("127.0.0.1", "\ud800")
except UnicodeEncodeError as exc:
    assert exc.encoding == "utf-8"
else:
    raise AssertionError("surrogate service was accepted")

loopback = "127.0.0.1"
assert socket.gethostbyname(bytearray(b"127.0.0.1")) == loopback
name, aliases, addresses = socket.gethostbyaddr(loopback)
assert name and isinstance(aliases, list) and loopback in addresses

assert socket.herror is _socket.herror
try:
    raise socket.herror(1, "host lookup failed")
except socket.gaierror:
    raise AssertionError("herror matched its sibling gaierror")
except OSError as exc:
    assert type(exc) is socket.herror and exc.errno == 1

assert getservbyname is _socket.getservbyname
assert raw_getservbyport is socket.getservbyport
try:
    dns_port = getservbyname("domain", "udp")
except OSError:
    pass  # Service database entries vary by host.
else:
    canonical = raw_getservbyport(dns_port, "udp")
    assert getservbyname(canonical, "udp") == dns_port
try:
    socket.getservbyport(-1, None)
except TypeError:
    pass
else:
    raise AssertionError("the protocol must be parsed before checking port bounds")
