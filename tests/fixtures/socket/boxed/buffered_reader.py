import io
import socket


sock, peer = socket.socketpair()
with sock, peer:
    sock.settimeout(2)
    peer.settimeout(2)
    fd = sock.fileno()
    with sock.makefile("rb", buffering=4) as reader:
        assert isinstance(reader, io.BufferedReader)
        assert isinstance(reader.raw, socket.SocketIO)
        sock.close()
        assert reader.fileno() == fd
        peer.sendall(b"abc\ndefgh")
        peer.shutdown(socket.SHUT_WR)
        assert reader.peek(1) == b"abc\n"
        assert reader.readline(2) == b"ab"
        assert reader.readline() == b"c\n"
        data = bytearray(3)
        assert reader.readinto(data) == 3 and data == b"def"
        assert reader.read() == b"gh"
        assert reader.read(1) == b"" and reader.readinto(data) == 0
    reader.close()
    assert reader.closed and reader.raw.closed and sock.fileno() == -1
    assert peer.recv(1) == b""
