import sys
sys.path

import socket
import socketserver

modes = ["normal"]
handlers = []
events = []

class Hook(socketserver.StreamRequestHandler):
    def handle(self):
        handlers.append(self)
        writer = self.wfile
        self.original_flush = writer.flush
        phase = modes[0]
        seen = []
        def flush():
            seen.append(1)
            events.append("flush")
            if (phase == "first-os" and len(seen) == 1) or (phase == "second-os" and len(seen) == 2):
                raise OSError("wire")
            if phase == "value" or phase == "direct-value":
                raise ValueError("unexpected")
            self.request.sendall(b"F")
        if phase != "unmodified":
            writer.flush = flush
        writer.write(b"B")
        if phase == "closed" or phase == "direct-value":
            try:
                writer.close()
            except ValueError as error:
                events.append(str(error))
            writer.close()

class Quiet(socketserver.TCPServer):
    def handle_error(self, request, address):
        events.append("error")
        request.sendall(b"E")

with Quiet(("127.0.0.1", 0), Hook) as server:
    for phase in ("unmodified", "normal", "closed", "first-os", "second-os", "value", "direct-value", "normal"):
        start = len(events)
        modes[0] = phase
        with socket.socket() as peer:
            peer.connect(server.server_address)
            server.handle_request()
            handler = handlers[-1]
            with peer.makefile("rb") as reply:
                print(phase, reply.read(), events[start:], handler.wfile.closed, handler.rfile.closed)
            if not handler.wfile.closed:
                handler.wfile.flush = handler.original_flush
                handler.wfile.close()
            handler.rfile.close()
