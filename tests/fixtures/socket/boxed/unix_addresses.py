"""Use real Unix sockets with filesystem and Linux abstract addresses."""
import errno
import os
import socket
import sys
import tempfile


with tempfile.TemporaryDirectory(dir="/tmp") as directory:
    path = directory + "/stream-\u00e9"
    peer = os.fsencode(directory) + b"/client-\xff"
    with socket.socket(socket.AF_UNIX) as server, socket.socket(socket.AF_UNIX) as client:
        assert server.getsockname() == ""
        server.bind(path)
        server.listen()
        client.bind(peer)
        try:
            client.connect(path)
            conn, address = server.accept()
            with conn:
                assert os.fsencode(address) == peer
                assert conn.getpeername() == address == client.getsockname()
                assert conn.getsockname() == client.getpeername() == path
                client.sendall(b"stream")
                assert conn.recv(6) == b"stream"
        finally:
            os.unlink(peer)

    path = directory + "/dgram"
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
        server.bind(memoryview(os.fsencode(path)))
        assert client.sendto(b"unbound", path) == 7
        payload, source = server.recvfrom(16)
        assert payload == b"unbound"
        if sys.platform.startswith("linux"):
            assert source is None
        try:
            client.bind(path)
        except OSError as exc:
            assert exc.errno == errno.EADDRINUSE
        else:
            raise AssertionError("second bind unexpectedly succeeded")
        client.bind(directory + "/sender")
        assert client.sendto(b"bound", path) == 5
        assert server.recvfrom(16) == (b"bound", client.getsockname())

    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sock:
        try:
            sock.bind(b"p" * 200)
        except OSError as exc:
            assert exc.args == ("AF_UNIX path too long",)
        else:
            raise AssertionError("overlong pathname accepted")
        try:
            sock.bind((path,))
        except TypeError:
            pass
        else:
            raise AssertionError("AF_UNIX accepted a tuple")

    if sys.platform.startswith("linux"):
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
            abstract = b"\0" + os.fsencode(directory) + b"\0tail\0"
            server.bind(abstract)
            assert server.getsockname() == abstract
            client.bind("")
            source = client.getsockname()
            assert isinstance(source, bytes) and source.startswith(b"\0") and len(source) > 1
            assert client.sendto(b"", abstract) == 0
            assert server.recvfrom(1) == (b"", source)
