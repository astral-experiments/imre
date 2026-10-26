import errno
import socket
import sys


def main() -> None:
    with socket.socket() as client:
        client.connect(("127.0.0.1", int(sys.argv[2])))
        if sys.argv[1] == "reset":
            client.sendall(b"!")
            try:
                client.recv(1)
            except ConnectionResetError as e:
                assert e.errno == errno.ECONNRESET
            else:
                raise AssertionError("peer reset reported as EOF")
            try:
                client.sendall(b"x")
            except ConnectionError as e:
                assert e.errno == errno.EPIPE or e.errno == errno.ECONNRESET
            else:
                raise AssertionError("sendall succeeded after disconnect")
        else:
            client.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
            block = b"\x00\xffYX"
            for i in range(17):
                block += block
            client.sendall(block)
    print("client passed")


main()
