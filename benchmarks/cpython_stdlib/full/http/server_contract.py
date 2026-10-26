"""Exercise HTTP handler subclass hooks, header shapes, status keys and delayed binding."""
import sys
import socket
import socketserver
from typing import Callable
from http import HTTPStatus
from http.client import HTTPMessage
from http.server import BaseHTTPRequestHandler, HTTPServer

lookups: list[str] = []


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    @property
    def server_version(self) -> str:
        return 'Overridden/10'

    def setup(self) -> None:
        try:
            self.requestline
            raise AssertionError('eager requestline')
        except AttributeError:
            pass
        try:
            self._headers_buffer
            raise AssertionError('eager buffer')
        except AttributeError:
            pass
        super().setup()

    def __getattr__(self, name: str) -> Callable[[], None]:
        if name.startswith('do_'):
            lookups.append(name)
            return self.run_wsgi
        raise AttributeError(name)

    def run_wsgi(self) -> None:
        command = self.command
        assert command == 'GET' or command == 'CHECK'
        assert lookups[-1] == lookups[-2]
        headers = self.headers
        if isinstance(headers, dict):
            assert not headers
            headers['lower'] = '0.9'
            assert self.headers.get('lower') == '0.9'
        else:
            assert isinstance(headers, HTTPMessage)
            values = headers.get_all('X-Value')
            assert values is not None
            assert values == ['one', 'two']
            assert self.headers.get('x-value') == 'one'
            assert self.headers.get('missing', 'default') == 'default'
        body = ''
        for key, value in self.headers.items():
            if key.lower() in ('x-value', 'lower'):
                body += key + '=' + value + '\n'
        self.send_response(HTTPStatus.OK)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(body.encode())


assert issubclass(HTTPServer, socketserver.TCPServer)
assert issubclass(BaseHTTPRequestHandler, socketserver.StreamRequestHandler)
assert BaseHTTPRequestHandler.MessageClass is HTTPMessage
assert HTTPServer is not BaseHTTPRequestHandler
assert not issubclass(socketserver.TCPServer, HTTPServer)
responses = BaseHTTPRequestHandler.responses
assert len(responses) == 62
for member in responses:
    assert HTTPStatus(member.value) is member
    assert responses[member.value] == (member.phrase, member.description)
assert responses[HTTPStatus.REQUEST_ENTITY_TOO_LARGE] == responses[413]
server = HTTPServer(('127.0.0.1', 0), Handler, False)
try:
    server.server_port
    raise AssertionError('eager binding')
except AttributeError:
    pass
server.server_bind()
assert server.server_port == server.server_address[1]
assert server.server_name == socket.getfqdn(server.server_address[0])
server.server_activate()
print(server.server_port)
sys.stdout.flush()
server.handle_request()
server.server_close()
