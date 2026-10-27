import sys
sys.path

import socket
import socketserver

class Reader:
    def __init__(self, events):
        self.events = events

    def close(self):
        self.events.append("reader")

class Writer:
    def __init__(self, sock, events):
        self.sock = sock
        self.events = events
        self.closed = False

    def write(self, data):
        self.sock.sendall(data)

    def flush(self):
        pass

    def close(self):
        self.closed = True
        self.events.append("writer")

class Handler(socketserver.StreamRequestHandler):
    def setup(self):
        assert not hasattr(self, "rfile") and not hasattr(self, "wfile")
        super().setup()
        self.original_reader = self.rfile
        self.original_writer = self.wfile

    def handle(self):
        self.rfile = None
        self.wfile = None
        assert self.rfile is None and self.wfile is None
        self.rfile = Reader(events)
        self.wfile = Writer(self.request, events)
        self.wfile.write(b"reply")

    def finish(self):
        try:
            super().finish()
        finally:
            self.original_reader.close()
            self.original_writer.close()

events = []
with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
    with socket.socket() as peer:
        peer.connect(server.server_address)
        server.handle_request()
        with peer.makefile("rb") as reply:
            assert reply.read() == b"reply"
assert events == ["writer", "reader"]
print("replacement streams closed")
