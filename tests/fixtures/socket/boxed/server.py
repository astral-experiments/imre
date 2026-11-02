import errno
import socket
from socket import create_server


assert create_server is socket.create_server
with create_server(("127.0.0.1", 0), backlog=2, reuse_port=True) as server:
    assert server.getsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT)
    address = server.getsockname()
    with socket.socket() as client:
        client.connect(address)
        peer, _ = server.accept()
        with peer:
            client.sendall(b"s")
            assert peer.recv(1) == b"s"
    try:
        create_server(address)
    except OSError as err:
        assert err.errno == errno.EADDRINUSE
        assert "while attempting to bind on address" in err.strerror
    else:
        raise AssertionError("second server bound the same address")
assert server.fileno() == -1
