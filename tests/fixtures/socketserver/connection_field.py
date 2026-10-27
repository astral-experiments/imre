"""Preserve the handler connection field even when explicitly assigned None."""
import sys
sys.path

import socket
import socketserver

class Alias(socketserver.StreamRequestHandler):
    def setup(self):
        print("before", hasattr(self, "connection"))
        super().setup()

    def handle(self):
        print("bind", self.connection is self.request)
        self.connection = None
        print("assigned", hasattr(self, "connection"))
        try:
            print("value", self.connection is None)
        except AttributeError as exc:
            print("missing", str(exc))
        self.connection = self.request
        print("restored", self.connection is self.request)
        self.connection.sendall(b"ok")

with socketserver.TCPServer(("127.0.0.1", 0), Alias) as server:
    with socket.socket() as peer:
        peer.connect(server.server_address)
        server.handle_request()
        print("reply", peer.recv(2))
