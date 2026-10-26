import errno
import socket
from socketserver import BaseRequestHandler, StreamRequestHandler, TCPServer


class Fails(BaseRequestHandler):
    def setup(self) -> None:
        self.request.sendall(b"S")

    def handle(self) -> None:
        self.request.sendall(b"H")
        raise ValueError("handler failed")

    def finish(self) -> None:
        self.request.sendall(b"F")


class Observed(TCPServer):
    allow_reuse_address = True
    request_queue_size = 7
    closes: int
    accepted: socket.socket | None

    def __init__(self, address: tuple[str, int], handler: type[BaseRequestHandler]) -> None:
        self.closes = 0
        self.accepted = None
        super().__init__(address, handler)

    def handle_error(self, request: socket.socket, client_address: tuple[str, int]) -> None:
        request.sendall(b"E")

    def close_request(self, request: socket.socket) -> None:
        self.accepted = request
        super().close_request(request)

    def server_close(self) -> None:
        self.closes += 1
        super().server_close()


class Rejected(Observed):
    def verify_request(self, request: socket.socket, client_address: tuple[str, int]) -> bool:
        return False


class Lines(StreamRequestHandler):
    disable_nagle_algorithm = True

    def handle(self) -> None:
        assert self.request.getsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY) != 0
        assert self.wfile.writable() and self.wfile.fileno() == self.request.fileno()
        assert self.timeout is None and self.rbufsize == -1 and self.wbufsize == 0
        assert self.rfile.readline(0) == b""
        first = self.rfile.readline(2)
        rest = self.rfile.readline()
        second = self.rfile.readline()
        tail = self.rfile.readline()
        assert self.rfile.readline() == b""
        assert self.wfile.write(first + b"|" + rest + second + tail) == 9
        self.wfile.flush()


def reply(server: TCPServer, data: bytes) -> bytes:
    wire = b""
    with socket.socket() as peer:
        peer.connect(server.server_address)
        peer.sendall(data)
        peer.shutdown(socket.SHUT_WR)
        server.handle_request()
        while True:
            part = peer.recv(16)
            if not part:
                break
            wire += part
    return wire


def main() -> None:
    server = Observed(("127.0.0.1", 0), Fails)
    try:
        with server as bound:
            assert bound is server
            assert server.socket.getsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR) != 0
            assert reply(server, b"") == b"SHFE"
            saved = server.accepted
            assert saved is not None and saved.fileno() == -1
            raise ValueError("unwind")
    except ValueError as exc:
        assert str(exc) == "unwind"
    assert server.socket.fileno() == -1 and server.closes == 1
    with Rejected(("127.0.0.1", 0), Fails) as reject:
        assert reply(reject, b"") == b""
        saved = reject.accepted
        assert saved is not None and saved.fileno() == -1
    delayed = TCPServer(("127.0.0.1", 0), Lines, bind_and_activate=False)
    with delayed:
        assert delayed.timeout is None and not delayed.allow_reuse_port
        assert delayed.server_address[1] == 0
        delayed.server_bind()
        delayed.server_activate()
        try:
            TCPServer(delayed.server_address, Lines)
        except OSError as exc:
            assert exc.errno == errno.EADDRINUSE
        else:
            raise AssertionError("a second listener acquired the same port")
        assert reply(delayed, b"abc\nxy\nz") == b"ab|c\nxy\nz"
    assert delayed.fileno() == -1
    try:
        delayed.handle_request()
    except ValueError as exc:
        assert str(exc) == "Invalid file descriptor: -1"
    else:
        raise AssertionError("closed listener accepted a request")
    print("handler lifecycle, rejected requests, stream read-ahead, bind and fd cleanup")


main()
