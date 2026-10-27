"""Original descriptor lifetime and a single buffered cursor across reads."""
import socket


def reject_negative_read(file, length):
    try:
        file.read(length)
    except ValueError as error:
        assert error.args == ("read length must be non-negative or -1",)
    else:
        raise AssertionError("invalid read consumed buffered input")


def run():
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
            assert file.read(4) == b"tail" and file.readinto(data) == 0
            assert file.read(None) == b""
            file.close()
            reject_negative_read(file, -2)
            assert file.closed and peer.fileno() == -1
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
