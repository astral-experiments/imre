"""Runtime-imported AF_INET operations used by the synchronous WSGI transport."""
import errno
import _socket
import socket

def check_bind_error(server):
    with socket.socket() as other:
        try:
            other.bind(server)
        except OSError as exc:
            assert exc.errno == errno.EADDRINUSE and exc.strerror
            assert exc.args == (exc.errno, exc.strerror)
        else:
            raise AssertionError("second socket bound the live listening port")

def run():
    sock = socket.socket()
    try:
        assert socket.close(sock.fileno()) is None
    finally:
        try:
            sock.close()
        except OSError as exc:
            assert exc.errno == errno.EBADF
        else:
            raise AssertionError("socket descriptor was still open")
    assert socket.has_ipv6 is True
    assert socket.AF_INET6 != socket.AF_INET
    try:
        socket.getaddrinfo("127.0.0.1", 0, socket.AF_INET, 77777)
    except socket.gaierror as exc:
        assert isinstance(exc, OSError) and exc.errno and exc.strerror
        assert exc.args == (exc.errno, exc.strerror)
    else:
        raise AssertionError("resolver accepted an invalid socket type")
    passive = socket.getaddrinfo(None, 0, socket.AF_INET, socket.SOCK_STREAM, flags=socket.AI_PASSIVE)
    assert passive[0][4] == ("0.0.0.0", 0)
    results = socket.getaddrinfo("127.0.0.1", 0, socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP)
    family, kind, protocol, canonical, addr = results[0]
    assert (family, kind, protocol) == (socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP)
    assert canonical == "" and addr == ("127.0.0.1", 0)
    with socket.socket(family, kind, protocol) as listener, socket.socket() as client:
        assert type(listener) is type(client) is socket.socket
        assert socket.SocketType is _socket.SocketType is _socket.socket
        assert isinstance(listener, socket.SocketType) and not isinstance(1, socket.SocketType)
        level, option = socket.SOL_SOCKET, socket.SO_REUSEADDR
        assert listener.getsockopt(level, option) == 0
        listener.setsockopt(level, option, 1)
        value = listener.getsockopt(level, option, 0)
        assert value != 0
        raw = listener.getsockopt(level, option, 1024)
        assert isinstance(raw, bytes) and len(raw) == 4 and any(raw)
        for length in (-1, 1025):
            try:
                listener.getsockopt(level, option, length)
            except OSError as exc:
                assert exc.args == ("getsockopt buflen out of range",)
            else:
                raise AssertionError("getsockopt accepted an invalid buffer size")
        listener.bind(addr)
        listener.listen(128)
        server = listener.getsockname()
        assert server[0] == "127.0.0.1" and server[1] > 0
        check_bind_error(server)
        client.connect(server)
        peer, address = listener.accept()
        with peer:
            assert isinstance(peer, socket.socket)
            assert peer.getsockname() == server and address == client.getsockname()
            assert peer.fileno() >= 0
            alias = {peer.fileno(): peer}[peer.fileno()]
            assert alias is peer
            peer.setblocking(False)
            try:
                peer.recv(1)
            except BlockingIOError:
                pass
            else:
                raise AssertionError("idle nonblocking socket was readable")
            peer.setblocking(True)
            client.sendall(b"\x00\xffGET")
            client.shutdown(socket.SHUT_WR)
            received = peer.recv(8192)
            assert received == b"\x00\xffGET" and isinstance(received, bytes)
            assert peer.recv(1) == b""
            assert peer.send(b"!") == 1
            peer.sendall(received)
            peer.shutdown(socket.SHUT_WR)
            for byte in (b"!", b"\x00", b"\xff", b"G", b"E", b"T"):
                assert client.recv(1) == byte
            assert client.recv(1) == b""
        assert alias.fileno() == -1
        try:
            alias.getsockname()
        except OSError:
            pass
        else:
            raise AssertionError("closed socket still had an endpoint")
    assert listener.fileno() == -1 and client.fileno() == -1
    try:
        listener.getsockopt(level, option)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("getsockopt succeeded on a closed socket")
    print("boxed socket transport passed")

run()
