"""Exchange connected UDP datagrams and preserve socket type after close."""
import errno
import socket


def check(family, host):
    with socket.socket(family, socket.SOCK_DGRAM) as sender, socket.socket(family, socket.SOCK_DGRAM) as receiver:
        sender.bind((host, 0))
        receiver.bind((host, 0))
        sender.connect(receiver.getsockname())
        receiver.connect(sender.getsockname())
        assert sender.type == receiver.type == socket.SOCK_DGRAM
        sender.sendall(b"first")
        assert sender.send(b"") == 0
        sender.sendall(b"last")
        receiver.setblocking(False)
        assert receiver.recv(2) == b"fi"
        assert receiver.recv(16) == b""
        assert receiver.recv(16) == b"last"
    assert sender.type == socket.SOCK_DGRAM and sender.fileno() == -1
    try:
        sender.send(b"x")
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("closed datagram socket sent data")


check(socket.AF_INET, "127.0.0.1")
try:
    check(socket.AF_INET6, "::1")
except OSError as exc:
    if exc.errno not in (errno.EAFNOSUPPORT, errno.EADDRNOTAVAIL, errno.EPROTONOSUPPORT):
        raise
