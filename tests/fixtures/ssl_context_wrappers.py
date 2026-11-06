import os
import socket
import ssl
import sys


class ClientContext(ssl.SSLContext):
    def __init__(self, protocol, *, minimum_version):
        self.minimum_version = minimum_version


client = ClientContext(ssl.PROTOCOL_TLS_CLIENT, minimum_version=ssl.TLSVersion.TLSv1_3)
assert client.minimum_version == ssl.TLSVersion.TLSv1_3
assert client.check_hostname and client.verify_mode == ssl.CERT_REQUIRED


context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
other = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
assert context.sslsocket_class is other.sslsocket_class is ssl.SSLSocket
assert context.sslobject_class is other.sslobject_class is ssl.SSLObject

allocations = []


class Socket(ssl.SSLSocket):
    def __new__(cls, **kwargs):
        allocations.append(kwargs.copy())
        return super().__new__(cls, **kwargs)

    def __init__(self, *args, **kwargs):
        raise AssertionError("wrap_socket called the public constructor")


class Object(ssl.SSLObject):
    pass


context.sslsocket_class = Socket
context.sslobject_class = Object
with socket.socket() as raw:
    fd = raw.fileno()
    with context.wrap_socket(raw, server_side=True) as peer:
        assert type(peer) is Socket and peer.context is context
        assert peer.fileno() == fd and raw.fileno() == -1
    assert peer.fileno() == -1
assert allocations == [dict(family=socket.AF_INET, type=socket.SOCK_STREAM, proto=0, fileno=fd)]
assert type(context.wrap_bio(ssl.MemoryBIO(), ssl.MemoryBIO(), server_side=True)) is Object
assert other.sslsocket_class is ssl.SSLSocket and other.sslobject_class is ssl.SSLObject


class Factory:
    @classmethod
    def _create(cls, **kwargs):
        return kwargs


context.sslsocket_class = Factory
with socket.socket() as raw:
    args = context.wrap_socket(raw, server_hostname="example.org")
    assert args == dict(sock=raw, server_side=False, do_handshake_on_connect=True,
                        suppress_ragged_eofs=True, server_hostname="example.org",
                        context=context, session=None)
    assert raw.fileno() >= 0
del context.sslsocket_class
assert context.sslsocket_class is ssl.SSLSocket

with socket.socket(type=socket.SOCK_DGRAM) as raw:
    try:
        ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT).wrap_socket(raw)
    except NotImplementedError as exc:
        assert str(exc) == "only stream sockets are supported"
    else:
        raise AssertionError("wrapped a datagram socket")
    assert raw.fileno() >= 0


def capture_wrapped_socket():
    with socket.socket() as raw:
        peer = context.wrap_socket(raw, server_side=True)
    return sys._getframe(), peer.fileno()


frame, fd = capture_wrapped_socket()
os.fstat(fd)
frame.clear()
try:
    os.fstat(fd)
except OSError:
    pass
else:
    raise AssertionError("clearing the frame left the TLS descriptor open")
