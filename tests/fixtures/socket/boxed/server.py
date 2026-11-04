import errno
import select
import socket
import sys
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
    with socket.socket() as client:
        status = client.connect_ex(address)
        assert type(status) is int and status == 0
        peer, source = server.accept()
        with peer:
            assert source == client.getsockname()
    if sys.platform == "linux":
        # Darwin can drop the SYN while a TCP port is bound but not listening.
        with socket.socket() as reserved, socket.socket() as refused:
            reserved.bind(("127.0.0.1", 0))
            status = refused.connect_ex(reserved.getsockname())
            assert type(status) is int and status == errno.ECONNREFUSED
        kind = socket.SOCK_STREAM | socket.SOCK_NONBLOCK | socket.SOCK_CLOEXEC
        with socket.socket(socket.AF_INET, kind) as client:
            assert client.type == socket.SOCK_STREAM
            assert client.gettimeout() == 0.0 and not client.get_inheritable()
            try:
                client.connect(address)
            except BlockingIOError as exc:
                assert exc.errno == errno.EINPROGRESS
            assert select.select([], [client], [], 2)[1] == [client]
            assert client.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR) == 0
            peer, _ = server.accept()
            with peer:
                peer.sendall(b"r")
                assert select.select([client], [], [], 2)[0] == [client]
                assert client.recv(1) == b"r"
    try:
        create_server(address)
    except OSError as err:
        assert err.errno == errno.EADDRINUSE
        assert "while attempting to bind on address" in err.strerror
    else:
        raise AssertionError("second server bound the same address")
assert server.fileno() == -1
