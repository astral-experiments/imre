import _socket
import socket

assert socket.inet_ntoa is _socket.inet_ntoa
assert socket.inet_ntop is _socket.inet_ntop
assert socket.inet_ntoa(memoryview(b"\x01\x00\x01\x00")) == "1.0.1.0"
assert socket.inet_ntop(socket.AF_INET, bytearray(b"\xaa\x55\xaa\x55")) == "170.85.170.85"
assert socket.inet_ntop(socket.AF_INET6, b"\x00" * 15 + b"\x01") == "::1"
