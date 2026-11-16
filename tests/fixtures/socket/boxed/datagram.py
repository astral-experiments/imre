"""Exchange UDP datagrams and preserve socket type after close."""
import errno
import struct
import socket
from _socket import MSG_PEEK as raw_peek, MSG_TRUNC as raw_trunc
from socket import MSG_PEEK, MSG_TRUNC, MsgFlag



def check(family, host):
    with socket.socket(family, socket.SOCK_DGRAM) as sender, socket.socket(family, socket.SOCK_DGRAM) as receiver:
        sender.bind((host, 0))
        receiver.bind((host, 0))
        source = sender.getsockname()
        target = receiver.getsockname()
        if family == socket.AF_INET6:
            receiver.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_RECVHOPLIMIT, 1)
            receiver.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_RECVTCLASS, 1)
            hop = struct.pack("i", 37)
            traffic = struct.pack("i", 32)
            controls = [(socket.IPPROTO_IPV6, socket.IPV6_HOPLIMIT, hop),
                        (socket.IPPROTO_IPV6, socket.IPV6_TCLASS, traffic)]
            assert sender.sendmsg([b"v6"], controls, 0, target) == 2
            data, ancillary, flags, peer = receiver.recvmsg(2, 2 * socket.CMSG_SPACE(len(hop)))
            assert data == b"v6" and peer == source and not flags & socket.MSG_CTRUNC
            assert sorted(ancillary) == sorted(controls)
        args = (b"extra", 0, target, 4)
        for kwargs, expected in (
            ({}, "sendto() takes 2 or 3 arguments (4 given)"),
            ({"flags": 0}, "sendto() takes no keyword arguments"),
        ):
            try:
                sender.sendto(*args, **kwargs)
            except TypeError as exc:
                assert str(exc) == expected
            else:
                raise AssertionError("accepted invalid sendto arguments")

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
        assert receiver.recvfrom(2, socket.MSG_PEEK) == (b"fi", source)
        assert receiver.recvfrom(16, MSG_PEEK) == (b"first", source)
        assert receiver.recvfrom(2) == (b"fi", source)
        assert receiver.recvfrom(16, MSG_PEEK) == (b"", source)
        assert receiver.recvfrom(16) == (b"", source)
        # A zero-size recvfrom consumes a datagram and still returns the sender.
        assert receiver.recvfrom(0) == (b"", source)
        assert receiver.recvfrom(16, 0) == (b"last", source)

        assert sender.sendto(b"queued", target) == 6
        out = bytearray(b"....")

        class Flags:
            def __index__(self):
                try:
                    out.append(0)
                except BufferError:
                    raise LookupError("invalid flag")
                raise AssertionError("recvfrom_into released the buffer before converting flags")

        try:
            receiver.recvfrom_into(out, len(out) + 1, Flags())
        except LookupError:
            pass
        else:
            raise AssertionError("accepted invalid flags")
        out.append(0)
        assert receiver.recvfrom_into(memoryview(out)[1:3], flags=0) == (2, source)
        assert out == b".qu.\x00"
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


assert type(raw_peek) is type(raw_trunc) is int
assert MSG_PEEK is MsgFlag.MSG_PEEK and MSG_TRUNC is MsgFlag.MSG_TRUNC
assert int(MSG_PEEK | MSG_TRUNC) == raw_peek | raw_trunc

check(socket.AF_INET, "127.0.0.1")
try:
    check(socket.AF_INET6, "::1")
except OSError as exc:
    if exc.errno not in (errno.EAFNOSUPPORT, errno.EADDRNOTAVAIL, errno.EPROTONOSUPPORT):
        raise


with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as original:
    original.bind(("127.0.0.1", 0))
    address = original.getsockname()
    with socket.socket(fileno=original.fileno()) as adopted:
        assert original.detach() == adopted.fileno()
        assert original.fileno() == -1
        assert adopted.getsockname() == address
        assert adopted.type == socket.SOCK_DGRAM
        adopted.sendto(b"adopted", address)
        assert adopted.recvfrom(32) == (b"adopted", address)
