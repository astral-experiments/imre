import socket
import socketserver


class Echo(socketserver.BaseRequestHandler):
    def setup(self) -> None:
        self.request.sendall(b"[")

    def handle(self) -> None:
        assert self.client_address[0] == "127.0.0.1"
        assert self.server.fileno() >= 0
        self.request.sendall(self.request.recv(64))

    def finish(self) -> None:
        self.request.sendall(b"]")


class Reverse(Echo):
    def handle(self):
        self.request.sendall(self.request.recv(64)[::-1])


def exchange(server: socketserver.TCPServer, expected: bytes) -> None:
    with socket.socket() as peer:
        peer.connect(server.server_address)
        peer.sendall(b"ab\x00\xff")
        server.handle_request()
        reply = b""
        while True:
            chunk = peer.recv(64)
            if not chunk:
                break
            reply += chunk
        assert reply == expected


def main() -> None:
    one = socketserver.TCPServer(("127.0.0.1", 0), Echo)
    two = socketserver.TCPServer(("127.0.0.1", 0), Reverse)
    with socketserver.TCPServer(("127.0.0.1", 0), Echo) as same:
        assert one.RequestHandlerClass is Echo
        assert Reverse is two.RequestHandlerClass
        assert one.RequestHandlerClass is same.RequestHandlerClass
        assert one.RequestHandlerClass is not two.RequestHandlerClass
    assert one.server_address != two.server_address
    exchange(one, b"[ab\x00\xff]")
    exchange(two, b"[\xff\x00ba]")
    one.server_close()
    two.server_close()
    assert one.fileno() == -1 and two.fileno() == -1
    print("two handler classes, real TCP, binary replies and EOF")


main()
