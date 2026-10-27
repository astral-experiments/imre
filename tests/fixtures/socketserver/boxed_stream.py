"""Exercise the runtime-loaded handler on two accepted connections."""
import sys
sys.path

import selectors
import socket
from socketserver import StreamRequestHandler, TCPServer


class Stop(Exception):
    pass


class Handler(StreamRequestHandler):
    def setup(self):
        assert not hasattr(self, "connection")
        super().setup()
        assert self.connection is self.request

    def handle(self):
        with selectors.DefaultSelector() as selector:
            key = selector.register(self.connection, selectors.EVENT_READ)
            [(ready, events)] = selector.select(timeout=0.01)
            assert ready is key and ready.fileobj is self.request
            assert events == selectors.EVENT_READ
        assert self.rfile.readline() == b"body\r\n"
        assert self.rfile.read(2) == b"a\x00"
        tail = bytearray(2)
        assert self.rfile.readinto(tail) == 2 and tail == b"\xffz"
        assert self.rfile.read(1) == b""
        assert self.wfile.write(b"ok\n") == 3


class Once(TCPServer):
    def serve_forever(self, poll_interval: float = 0.5) -> None:
        super().serve_forever(poll_interval=poll_interval)

    def service_actions(self):
        raise Stop()


with Once(("127.0.0.1", 0), Handler) as server:
    for explicit in (False, True):
        with socket.socket() as peer:
            peer.connect(server.server_address)
            peer.sendall(b"body\r\na\x00\xffz")
            peer.shutdown(socket.SHUT_WR)
            try:
                if explicit:
                    server.serve_forever(poll_interval=0.01)
                else:
                    server.serve_forever()
            except Stop:
                pass
            with peer.makefile("rb") as reply:
                assert reply.readline() == b"ok\n" and reply.read(1) == b""
    server.shutdown()
assert server.fileno() == -1
print("two accepted streams and selector keys, default and keyword float intervals")
