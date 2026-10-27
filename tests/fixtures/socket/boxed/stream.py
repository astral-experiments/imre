"""Original descriptor lifetime and a single buffered cursor across reads."""
import io
import socket


def reject_negative_read(file, length):
    try:
        file.read(length)
    except ValueError as error:
        assert error.args == ("read length must be non-negative or -1",)
    else:
        raise AssertionError("invalid read consumed buffered input")


class UnsupportedOperation(Exception):
    pass


def check_tell(stream):
    try:
        stream.tell()
    except UnsupportedOperation:
        raise AssertionError("socket error matched a user class with the same name")
    except io.UnsupportedOperation as exc:
        assert exc.args == ("seek",) and type(exc) is io.UnsupportedOperation
        assert isinstance(exc, OSError) and isinstance(exc, ValueError)
    else:
        raise AssertionError("buffered socket reported a position")


class ClosingWhence:
    def __init__(self, stream):
        self.stream = stream

    def __index__(self):
        self.stream.close()
        return 0


def run():
    with socket.socket() as peer:
        stream = peer.makefile("rb")
        try:
            stream.seek(0, ClosingWhence(stream))
        except ValueError as exc:
            assert exc.args == ("seek of closed file",)
            assert stream.closed
        else:
            raise AssertionError("seek succeeded after whence conversion closed the stream")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        with socket.socket() as client:
            client.connect(listener.getsockname())
            peer, _ = listener.accept()
            file = peer.makefile("rb", -1)
            second = peer.makefile("rb")
            descriptor = peer.fileno()
            assert file.fileno() == second.fileno() == descriptor
            peer.close()
            assert peer.fileno() == descriptor
            second.close()
            assert second.closed and not file.closed and peer.fileno() == descriptor
            client.sendall(b"Header\r\n\x00\xffENDtail")
            client.shutdown(socket.SHUT_WR)
            for length in (-2, -9):
                reject_negative_read(file, length)
            assert file.readline(4) == b"Head" and file.readline(None) == b"er\r\n"
            data = bytearray(5)
            alias = data
            assert file.readinto(data) == 5 and bytes(alias) == b"\x00\xffEND"
            check_tell(file)
            for origin in (0, 1, 2):
                try:
                    file.seek(0, origin)
                except io.UnsupportedOperation as exc:
                    assert exc.args == ("File or stream is not seekable.",)
                else:
                    raise AssertionError("buffered socket accepted seek")
            assert file.read(4) == b"tail" and file.readinto(data) == 0
            assert file.read(None) == b""
            file.close()
            reject_negative_read(file, -2)
            assert file.closed and peer.fileno() == -1
            check_tell(file)
            file.close()
        with socket.socket() as client:
            client.connect(listener.getsockname())
            peer, _ = listener.accept()
            with peer:
                file = peer.makefile("rb")
                file.close()
                assert peer.fileno() >= 0
                client.sendall(b"X\xff")
                data = bytearray(4)
                assert peer.recv_into(data, 2) == 2
                assert bytes(data) == b"X\xff\x00\x00"
    print("socket buffered input and close orders passed")


run()
