import _socket
import socket
import sys

assert socket.inet_ntoa is _socket.inet_ntoa
assert socket.inet_ntop is _socket.inet_ntop
assert socket.inet_ntoa(memoryview(b"\x01\x00\x01\x00")) == "1.0.1.0"
assert socket.inet_ntop(socket.AF_INET, bytearray(b"\xaa\x55\xaa\x55")) == "170.85.170.85"
assert socket.inet_ntop(socket.AF_INET6, b"\x00" * 15 + b"\x01") == "::1"


class Port:
    def __index__(self):
        return 0x1234


assert socket.htons is _socket.htons
assert socket.htons(Port()).to_bytes(2, sys.byteorder) == b"\x12\x34"
assert socket.ntohs(socket.htons(Port())) == 0x1234
assert socket.ntohl(socket.htonl(0x89ABCDEF)) == 0x89ABCDEF
for convert, width in ((socket.htons, 16), (socket.ntohl, 32)):
    try:
        convert(-1 << 1000)
    except ValueError:
        pass
    else:
        raise AssertionError("negative index accepted")
    try:
        convert(1 << width)
    except OverflowError:
        pass
    else:
        raise AssertionError("out-of-range index accepted")
