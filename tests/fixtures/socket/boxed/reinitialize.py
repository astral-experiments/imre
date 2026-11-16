"""Reinitializing a socket changes the endpoint observed by its existing SocketIO."""
import _socket
import errno
import os
import socket


def closed(fd):
    try:
        os.fstat(fd)
    except OSError as exc:
        assert exc.errno == errno.EBADF
        return True
    return False


def check_reader(initializer, another_reader):
    left, original_peer = socket.socketpair()
    replacement, peer = socket.socketpair()
    original_fd = left.fileno()
    new_fd = replacement.detach()
    calls = []
    readers = []

    class Number:
        def __index__(self):
            # Buffer ownership can change during argument conversion.
            readers.append(left.makefile("rb", buffering=0))
            calls.append(left.fileno())
            return new_fd

    with left, original_peer, peer:
        initializer(left, fileno=Number())
        try:
            assert calls == [original_fd]
            assert readers[0].fileno() == left.fileno() == new_fd
            os.write(original_fd, b"old")
            assert original_peer.recv(3) == b"old"
            peer.sendall(b"new")
            assert readers[0].read(3) == b"new"
            second = left.makefile("rb", buffering=0) if another_reader else None
            left.close()
            public = initializer is socket.socket.__init__
            assert closed(new_fd) == (public and second is None)
            readers[0].close()
            # Public __init__ reset the wrapper's counter while raw __init__ kept it.
            assert closed(new_fd) == (public or second is None)
            if second is not None:
                second.close()
            assert closed(new_fd) and peer.recv(1) == b""
        finally:
            os.close(original_fd)


def check_close_intent(initializer):
    left, first_peer = socket.socketpair()
    replacement, peer = socket.socketpair()
    new_fd = replacement.detach()
    with left, first_peer, peer:
        left.close()
        initializer(left, fileno=new_fd)
        left.makefile("rb", buffering=0).close()
        assert closed(new_fd) == (initializer is _socket.socket.__init__)
        left.close()
        assert closed(new_fd) and peer.recv(1) == b""


for init in (socket.socket.__init__, _socket.socket.__init__):
    check_reader(init, False)
    check_reader(init, True)
    check_close_intent(init)
