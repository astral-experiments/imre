"""Runtime-imported AF_INET operations used by the synchronous WSGI transport."""
import errno
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
    try:
        socket.getaddrinfo("127.0.0.1", 0, socket.AF_INET, 77777)
    except socket.gaierror as exc:
        assert isinstance(exc, OSError) and exc.errno and exc.strerror
        assert exc.args == (exc.errno, exc.strerror)
    else:
        raise AssertionError("resolver accepted an invalid socket type")
    results = socket.getaddrinfo("127.0.0.1", 0, socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP)
    family, kind, protocol, canonical, addr = results[0]
    assert (family, kind, protocol) == (socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP)
    assert canonical == "" and addr == ("127.0.0.1", 0)
    with socket.socket(family, kind, protocol) as listener, socket.socket() as client:
        assert type(listener) is type(client) is socket.socket
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
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
    print("boxed socket transport passed")

run()
