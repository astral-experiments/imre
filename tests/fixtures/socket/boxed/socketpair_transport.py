import errno
import socket
from socket import socketpair


left, right = socketpair()
with left, right:
    for endpoint in (left, right):
        assert endpoint.family == socket.AF_UNIX
        assert endpoint.type == socket.SOCK_STREAM and endpoint.proto == 0
        assert endpoint.fileno() >= 0 and not endpoint.get_inheritable()
        assert endpoint.getsockname() == endpoint.getpeername() == ""
    assert left.fileno() != right.fileno()
    left.sendall(b"left")
    assert right.recv(4) == b"left"
    right.sendall(b"right")
    assert left.recv(5) == b"right"
    left.shutdown(socket.SHUT_WR)
    assert right.recv(1) == b""
    left.close()
    assert left.fileno() == -1 and right.fileno() >= 0
    try:
        left.get_inheritable()
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("read descriptor flags after close")
assert right.fileno() == -1

left, right = socketpair(family=socket.AF_UNIX, type=socket.SOCK_DGRAM)
with left, right:
    left.send(b"first")
    left.send(b"")
    left.send(b"last")
    assert right.recv(2) == b"fi"
    assert right.recv(10) == b""
    assert right.recv(10) == b"last"
