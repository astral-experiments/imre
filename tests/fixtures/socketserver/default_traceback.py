"""Print a request's traceback to the current stderr and close its socket."""
import sys
sys.path

import io
import socket
import socketserver


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            raise LookupError("request cause")
        except LookupError as cause:
            raise ValueError("request failed") from cause


with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
    with socket.socket() as client:
        client.connect(server.server_address)
        stream = io.StringIO()
        previous = sys.stderr
        sys.stderr = stream
        try:
            server.handle_request()
        finally:
            sys.stderr = previous
        rendered = stream.getvalue()
        print("peer", repr(client.getsockname()) in rendered)
        print("dividers", rendered.startswith("----------------------------------------\n") and rendered.endswith("----------------------------------------\n"))
        print("tracebacks", "Traceback (most recent call last):" in rendered)
        print("frames", "in handle\n" in rendered)
        print("cause", "LookupError: request cause\n" in rendered)
        print("chain", "was the direct cause" in rendered)
        print("error", "ValueError: request failed\n" in rendered)
        print("closed", client.recv(1))

    stream = io.StringIO()
    sys.stderr = stream
    try:
        server.handle_error(server.socket, server.server_address)
    finally:
        sys.stderr = previous
    print("outside", "NoneType: None\n" in stream.getvalue())
