"""Exchange UDP datagrams and preserve socket type after close."""
import errno
import socket


def check(family, host):
    with socket.socket(family, socket.SOCK_DGRAM) as sender, socket.socket(family, socket.SOCK_DGRAM) as receiver:
        sender.bind((host, 0))
        receiver.bind((host, 0))
        source = sender.getsockname()
        target = receiver.getsockname()
        assert sender.sendto(memoryview(b"first"), target) == 5
        assert sender.sendto(b"", 0, target) == 0
        assert sender.sendto(bytearray(b"discard"), target) == 7
        payload = bytearray(b"last")
        class Port:
            def __index__(self):
                try:
                    payload.append(0)
                except BufferError:
                    return target[1]
                raise AssertionError("sendto released the buffer before converting the port")

        assert sender.sendto(payload, (host.encode("ascii"), Port())) == 4
        payload.append(0)
        assert receiver.recvfrom(2) == (b"fi", source)
        assert receiver.recvfrom(16) == (b"", source)
        # A zero-size recvfrom consumes a datagram and still returns the sender.
        assert receiver.recvfrom(0) == (b"", source)
        assert receiver.recvfrom(16, 0) == (b"last", source)
        receiver.setblocking(False)
        try:
            receiver.recvfrom(16)
        except BlockingIOError:
            pass
        else:
            raise AssertionError("recvfrom unexpectedly returned an empty packet")
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
