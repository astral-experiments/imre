"""Peer for the original TCPServer test's three synchronous wire exchanges."""
import socketserver
import sys


class Echo(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        line = self.rfile.readline()
        self.wfile.write(line)


class Wrong(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        self.rfile.readline()
        self.wfile.write(b"wrong\n")


def main() -> None:
    wrong = sys.argv[1] == "wrong"
    if wrong:
        server = socketserver.TCPServer(("127.0.0.1", 0), Wrong)
        count = 1
    else:
        server = socketserver.TCPServer(("127.0.0.1", 0), Echo)
        count = 3
    with server:
        assert server.socket.getsockname() == server.server_address
        print(server.server_address[1])
        sys.stdout.flush()
        for i in range(count):
            server.handle_request()
    print(server.socket.fileno())
    sys.stdout.flush()


main()
