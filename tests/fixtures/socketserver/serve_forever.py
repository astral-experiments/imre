import socket
import socketserver


class Stop(Exception):
    pass


class Echo(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        self.request.sendall(self.request.recv(64))


class Once(socketserver.TCPServer):
    def service_actions(self) -> None:
        raise Stop()


with Once(("127.0.0.1", 0), Echo) as server:
    # An idle listener runs the service hook without accepting a connection.
    try:
        server.serve_forever(poll_interval=0.01)
    except Stop:
        print("idle hook")
    with socket.socket() as peer:
        peer.connect(server.server_address)
        peer.sendall(b"probe\x00\xff")
        try:
            server.serve_forever(poll_interval=0.01)
        except Stop:
            pass
        assert peer.recv(64) == b"probe\x00\xff"
        assert peer.recv(1) == b""
    # The loop signalled completion on both exceptional exits.
    server.shutdown()
assert server.fileno() == -1
print("closed")
