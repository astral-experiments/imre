import io
import select
import socket


def check_writer(socket_first):
    sock, peer = socket.socketpair()
    with sock, peer:
        peer.settimeout(2)
        fd = sock.fileno()
        if socket_first:
            writer = sock.makefile("wb", buffering=8)
            sock.close()
        else:
            writer = sock.makefile("wb")
        with writer:
            assert isinstance(writer, io.BufferedWriter) and writer.fileno() == fd
            assert writer.write(b"A") == 1
            assert not select.select([peer], [], [], 0)[0]
            writer.flush()
            assert peer.recv(1) == b"A"
            assert writer.write(b"B") == 1
        assert peer.recv(1) == b"B"
        if not socket_first:
            assert sock.fileno() == fd
            sock.sendall(b"C")
            assert peer.recv(1) == b"C"
            sock.close()
        assert sock.fileno() == -1 and peer.recv(1) == b""


check_writer(socket_first=True)
check_writer(socket_first=False)

with socket.socket() as sock, sock.makefile("wb", -(1 << 80)) as writer:
    assert isinstance(writer, io.BufferedWriter)


with socket.socket() as sock:
    with sock.makefile("rb") as reader:
        assert reader.mode == "rb"
        fd = sock.fileno()
        try:
            sock.makefile("w", encoding="utf-8", newline="invalid")
        except ValueError:
            pass
        else:
            raise AssertionError("accepted an invalid newline")
        sock.close()
        assert sock.fileno() == fd
    assert reader.mode == "rb" and sock.fileno() == -1

sock, peer = socket.socketpair()
with sock, peer:
    peer.settimeout(2)
    sock.settimeout(2)
    with sock.makefile("rw", encoding="utf-8") as stream:
        sock.close()
        peer.sendall(b"request\n")
        assert stream.mode == "rw" and stream.readline() == "request\n"
        assert stream.write("!") == 1
    assert peer.recv(1) == b"!" and sock.fileno() == -1
