import importlib
import socket
import sys
from collections import UserString
from contextlib import redirect_stderr
from io import StringIO
from socketserver import ThreadingMixIn

server = importlib.import_module(sys.argv[1])
assert server.ThreadingHTTPServer.__bases__ == (ThreadingMixIn, server.HTTPServer)
assert server.ThreadingHTTPServer.daemon_threads is True

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

class RequestFormat(UserString):
    def translate(self, table):
        return "translated: " + str(super().translate(table))

logger = object.__new__(server.BaseHTTPRequestHandler)
logger.client_address = ("127.0.0.1", 0)
with redirect_stderr(StringIO()) as output:
    logger.log_error(RequestFormat("path=%s"), "/a\nb")
    assert output.getvalue().endswith("] translated: path=/a\\x0ab\n")
    try:
        logger.log_message(RequestFormat("%d"), "bad")
    except TypeError:
        pass
    else:
        raise AssertionError("expected an integer format argument")
assert len(output.getvalue().splitlines()) == 1
