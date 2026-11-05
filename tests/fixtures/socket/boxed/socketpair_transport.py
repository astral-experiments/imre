import errno
import os
import socket
from socket import fromfd, socketpair


left, right = socketpair()
with left, right:
    for endpoint in (left, right):
        assert endpoint.family == socket.AF_UNIX
        assert endpoint.type == socket.SOCK_STREAM and endpoint.proto == 0
        assert endpoint.fileno() >= 0 and not endpoint.get_inheritable()
        assert endpoint.getsockname() == endpoint.getpeername() == ""
    assert left.fileno() != right.fileno()
    assert left.set_inheritable(inheritable=True) is None
    assert os.get_inheritable(left.fileno())
    left.set_inheritable(False)
    assert not left.get_inheritable()
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

left, right = socketpair()
with right, left.dup() as copy:
    old_fd = left.fileno()
    left.close()
    # Reuse the exact number for the peer before closing and dropping the old wrapper.
    os.dup2(right.fileno(), old_fd, inheritable=False)
    try:
        left.close()
        del left
        os.write(old_fd, b"x")
        assert copy.recv(1) == b"x"
    finally:
        os.close(old_fd)

left, right = socketpair()
with left, right:
    left.settimeout(1)
    with fromfd(left.fileno(), left.family, left.type) as copy:
        assert copy.fileno() != left.fileno() and not copy.get_inheritable()
        assert copy.gettimeout() == socket.getdefaulttimeout()
        left.close()
        right.sendall(b"x")
        right.shutdown(socket.SHUT_WR)
        assert copy.recv(1) == b"x" and copy.recv(1) == b""

previous = socket.getdefaulttimeout()
try:
    socket.setdefaulttimeout(0.75)
    left, right = socketpair()
    with left, right:
        assert left.gettimeout() == right.gettimeout() == 0.75
        socket.setdefaulttimeout(None)
        with socket.socket() as other:
            assert other.gettimeout() is None and left.gettimeout() == 0.75
finally:
    socket.setdefaulttimeout(previous)
