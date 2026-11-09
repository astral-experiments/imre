import errno
import gc
import os
import socket
import ssl


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

# A public type alias is independent of the transport used by SSL.
original_type = socket.SocketType
with socket.socket() as raw:
    fd = raw.fileno()
    try:
        socket.SocketType = None
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        with context.wrap_socket(raw, server_side=True) as peer:
            assert raw.fileno() == -1 and peer.fileno() == fd
    finally:
        socket.SocketType = original_type

# Keep the wrapping exception alive while its caller reuses the descriptor number.
if os.name == "posix":
    def claim_number(fd):
        new_fd = os.open(os.devnull, os.O_RDONLY)
        if new_fd != fd:
            os.dup2(new_fd, fd)
            os.close(new_fd)

    class DeferredTimeout:
        def __index__(self):
            raise AssertionError("converted the timeout before detecting the invalid socket")

    class ReuseOnDetach(socket.socket):
        def gettimeout(self):
            return DeferredTimeout()

        def detach(self):
            fd = super().detach()
            os.close(fd)
            claim_number(fd)
            return fd

    left, right = socket.socketpair()
    with right, ReuseOnDetach(fileno=left.detach()) as raw:
        fd = raw.fileno()
        try:
            context.wrap_socket(raw, server_side=True, do_handshake_on_connect=False)
        except OSError as exc:
            assert exc.errno == errno.ENOTSOCK
            saved_error = exc
        else:
            raise AssertionError("wrapped a regular file")
        try:
            os.fstat(fd)
        except OSError as exc:
            assert exc.errno == errno.EBADF
        else:
            raise AssertionError("failed wrap kept the replacement open")
        claim_number(fd)
        try:
            assert saved_error.errno == errno.ENOTSOCK
            del saved_error
            gc.collect()
            assert os.read(fd, 1) == b""
        finally:
            os.close(fd)
        assert right.recv(1) == b""

    class TimeoutFailure(Exception):
        pass

    class BadTimeout:
        def __index__(self):
            raise TimeoutFailure

    class Raw(socket.socket):
        def gettimeout(self):
            return BadTimeout()

    class Peer(ssl.SSLSocket):
        def close(self):
            seen.append(self)

    class CleanupError(ssl.SSLSocket):
        def close(self):
            super().close()
            raise RuntimeError("close")

    context.sslsocket_class = Peer
    seen = []
    left, right = socket.socketpair()
    with right, Raw(fileno=left.detach()) as raw:
        fd = raw.fileno()
        try:
            context.wrap_socket(raw, server_side=True, do_handshake_on_connect=False)
        except TimeoutFailure:
            pass
        assert raw.fileno() == -1 and len(seen) == 1 and seen[0].fileno() == fd
        os.write(fd, b"x")
        assert right.recv(1) == b"x"
        ssl.SSLSocket.close(seen.pop())
        assert right.recv(1) == b""

    context.sslsocket_class = CleanupError
    left, right = socket.socketpair()
    with right, Raw(fileno=left.detach()) as raw:
        fd = raw.fileno()
        try:
            context.wrap_socket(raw, server_side=True, do_handshake_on_connect=False)
        except RuntimeError as exc:
            assert type(exc.__context__) is TimeoutFailure
        else:
            raise AssertionError("lost the virtual close error")
        claim_number(fd)
        try:
            gc.collect()
            assert os.read(fd, 1) == b""
        finally:
            os.close(fd)

    # A failed shutdown must not expose the base socket's plaintext connect.
    context.sslsocket_class = ssl.SSLSocket
    with socket.socket() as listener, socket.socket() as raw:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        with context.wrap_socket(raw, server_side=True) as peer:
            try:
                peer.shutdown(socket.SHUT_RD)
            except OSError as exc:
                assert exc.errno == errno.ENOTCONN
            for connect in (peer.connect, peer.connect_ex):
                try:
                    connect(listener.getsockname())
                except ValueError as exc:
                    assert str(exc) == "can't connect in server-side mode"
                else:
                    raise AssertionError("unprotected TCP connect on SSL socket")

    import sys

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

    # Only this wrap is subject to the policy. The input socket already exists.
    with socket.socket() as raw:
        audited = []
        def allow_tls(event, args):
            if event == "socket.__new__":
                if not isinstance(args[0], ssl.SSLSocket):
                    raise PermissionError("plaintext socket")
                audited.append(args[0])
        sys.addaudithook(allow_tls)
        with context.wrap_socket(raw, server_side=True) as peer:
            assert len(audited) == 1 and audited[0] is peer
