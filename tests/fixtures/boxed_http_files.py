import importlib
import socket
import sys

server = importlib.import_module(sys.argv[1])

class Handler(server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

client, request = socket.socketpair()
with client, request:
    client.sendall(b"GET /served/ HTTP/1.0\r\n\r\n")
    Handler(request, ("127.0.0.1", 0), None, directory=sys.argv[2])
    request.shutdown(socket.SHUT_WR)
    with client.makefile("rb") as response:
        headers, body = response.read().split(b"\r\n\r\n", 1)
assert headers.startswith(b"HTTP/1.0 200 ")
assert b'<a href="a%26b">a&amp;b</a>' in body
