"""Observe the kernel reset and the buffered reader state after catching it."""
import socket
import sys


for method in ("recv", "read", "readline", "readinto"):
    with socket.socket() as peer:
        peer.connect(("127.0.0.1", int(sys.argv[1])))
        if method == "recv":
            assert peer.recv(1) == b"\n"
        else:
            stream = peer.makefile("rb")
            assert stream.readline() == b"\n"
        peer.sendall(b"G")
        target = bytearray(b"-")
        try:
            if method == "recv":
                peer.recv(1)
            elif method == "read":
                stream.read(1)
            elif method == "readline":
                stream.readline()
            else:
                stream.readinto(target)
        except OSError as exc:
            print(method, type(exc).__name__, exc.args, getattr(exc, "errno", None),
                  getattr(exc, "strerror", None))
        else:
            raise AssertionError("expected a TCP reset")
        assert bytes(target) == b"-"
        if method == "recv":
            assert peer.recv(1) == b""
        else:
            assert stream.read(1) == b""
            stream.close()
