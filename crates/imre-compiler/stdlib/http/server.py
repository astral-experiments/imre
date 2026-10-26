"""Synchronous HTTP request handling on socketserver's TCP streams."""
from __future__ import annotations

import codecs
import socket
import socketserver
import sys
import time
from http import HTTPStatus
from http.client import HTTPMessage

__version__ = "0.6"
DEFAULT_ERROR_MESSAGE = """\
<!DOCTYPE HTML>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <style type="text/css">
            :root {
                color-scheme: light dark;
            }
        </style>
        <title>Error response</title>
    </head>
    <body>
        <h1>Error response</h1>
        <p>Error code: %(code)d</p>
        <p>Message: %(message)s.</p>
        <p>Error code explanation: %(code)s - %(explain)s.</p>
    </body>
</html>
"""

DEFAULT_ERROR_CONTENT_TYPE = "text/html;charset=utf-8"


def _control_codes() -> dict[int, str]:
    codes: dict[int, str] = {}
    for code in range(32):
        codes[code] = "\\x%02x" % code
    for code in range(127, 160):
        codes[code] = "\\x%02x" % code
    codes[92] = "\\\\"
    return codes


def _months() -> list[str | None]:
    return [None, "Jan", "Feb", "Mar", "Apr", "May", "Jun",
                 "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def _escape(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class HTTPServer(socketserver.TCPServer):
    allow_reuse_address = True
    allow_reuse_port = False
    _bound_name: str | None
    _bound_port: int | None

    def __init__(self, server_address: tuple[str, int], RequestHandlerClass: type[socketserver.BaseRequestHandler], bind_and_activate: bool = True) -> None:
        self._bound_name = None
        self._bound_port = None
        super().__init__(server_address, RequestHandlerClass, bind_and_activate)

    def server_bind(self) -> None:
        super().server_bind()
        host, port = self.server_address
        self._bound_name = socket.getfqdn(host)
        self._bound_port = port

    @property
    def server_name(self) -> str:
        name = self._bound_name
        if name is None:
            raise AttributeError("server_name")
        return name

    @server_name.setter
    def server_name(self, value: str) -> None:
        self._bound_name = value

    @property
    def server_port(self) -> int:
        port = self._bound_port
        if port is None:
            raise AttributeError("server_port")
        return port

    @server_port.setter
    def server_port(self, value: int) -> None:
        self._bound_port = value


class BaseHTTPRequestHandler(socketserver.StreamRequestHandler):
    sys_version = "Python/" + sys.version.split()[0]
    server_version = "BaseHTTP/" + __version__
    default_request_version = "HTTP/0.9"
    protocol_version = "HTTP/1.0"
    error_content_type = DEFAULT_ERROR_CONTENT_TYPE
    error_message_format = DEFAULT_ERROR_MESSAGE
    MessageClass = HTTPMessage
    _control_char_table = _control_codes()
    responses = {v: (v.phrase, v.description) for v in HTTPStatus.__members__.values()}
    weekdayname = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
    monthname = _months()
    # socketserver calls subclass hooks during construction. These values only
    # become visible when the matching HTTP operation first assigns them.
    _raw_requestline: bytes | None
    _requestline: str | None
    _request_version: str | None
    _command: str | None
    _path: str | None
    _headers: HTTPMessage | None
    _close_connection: bool | None
    _buffer: list[bytes] | None
    _command_set: bool

    def __init__(self, request: socket.socket, client_address: tuple[str, int], server: socketserver.TCPServer) -> None:
        self._raw_requestline = None
        self._requestline = None
        self._request_version = None
        self._command = None
        self._path = None
        self._headers = None
        self._close_connection = None
        self._buffer = None
        self._command_set = False
        super().__init__(request, client_address, server)

    @property
    def raw_requestline(self) -> bytes:
        value = self._raw_requestline
        if value is None:
            raise AttributeError("raw_requestline")
        return value

    @raw_requestline.setter
    def raw_requestline(self, value: bytes) -> None:
        self._raw_requestline = value

    @property
    def requestline(self) -> str:
        value = self._requestline
        if value is None:
            raise AttributeError("requestline")
        return value

    @requestline.setter
    def requestline(self, value: str) -> None:
        self._requestline = value

    @property
    def request_version(self) -> str:
        value = self._request_version
        if value is None:
            raise AttributeError("request_version")
        return value

    @request_version.setter
    def request_version(self, value: str) -> None:
        self._request_version = value

    @property
    def command(self) -> str | None:
        value = self._command
        if not self._command_set:
            raise AttributeError("command")
        return value

    @command.setter
    def command(self, value: str | None) -> None:
        self._command = value
        self._command_set = True

    @property
    def path(self) -> str:
        value = self._path
        if value is None:
            raise AttributeError("path")
        return value

    @path.setter
    def path(self, value: str) -> None:
        self._path = value

    @property
    def headers(self) -> HTTPMessage:
        value = self._headers
        if value is None:
            raise AttributeError("headers")
        return value

    @headers.setter
    def headers(self, value: HTTPMessage) -> None:
        self._headers = value

    @property
    def close_connection(self) -> bool:
        value = self._close_connection
        if value is None:
            raise AttributeError("close_connection")
        return value

    @close_connection.setter
    def close_connection(self, value: bool) -> None:
        self._close_connection = value

    @property
    def _headers_buffer(self) -> list[bytes]:
        value = self._buffer
        if value is None:
            raise AttributeError("_headers_buffer")
        return value

    @_headers_buffer.setter
    def _headers_buffer(self, value: list[bytes]) -> None:
        self._buffer = value

    def parse_request(self) -> bool:
        self.command = None
        self.request_version = self.default_request_version
        self.close_connection = True
        self.requestline = codecs.lookup("iso-8859-1").decode(self.raw_requestline)[0].rstrip("\r\n")
        words = self.requestline.split()
        if not words:
            return False
        if len(words) >= 3:
            version = words[-1]
            parts = version[5:].split(".")
            if (not version.startswith("HTTP/") or len(parts) != 2 or
                    not parts[0] or not parts[1] or
                    parts[0].strip("0123456789") or parts[1].strip("0123456789") or
                    len(parts[0]) > 10 or len(parts[1]) > 10):
                self.send_error(400, "Bad request version (" + repr(version) + ")")
                return False
            try:
                major = int(parts[0])
                minor = int(parts[1])
            except ValueError:
                self.send_error(400, "Bad request version (" + repr(version) + ")")
                return False
            if (major > 1 or (major == 1 and minor >= 1)) and self.protocol_version.encode() >= b"HTTP/1.1":
                self.close_connection = False
            if major >= 2:
                self.send_error(505, "Invalid HTTP version (" + version[5:] + ")")
                return False
            self.request_version = version
        if len(words) < 2 or len(words) > 3:
            self.send_error(400, "Bad request syntax (" + repr(self.requestline) + ")")
            return False
        if len(words) == 2 and words[0] != "GET":
            self.send_error(400, "Bad HTTP/0.9 request type (" + repr(words[0]) + ")")
            return False
        self.command = words[0]
        self.path = words[1]
        if self.path.startswith("//"):
            self.path = "/" + self.path.lstrip("/")
        if len(words) == 2:
            self.headers = HTTPMessage()
            return True
        lines: list[str] = []
        while True:
            line = self.rfile.readline(65537)
            if len(line) > 65536:
                self.send_error(431, "Line too long", "got more than 65536 bytes when reading header line")
                return False
            if len(lines) >= 100:
                self.send_error(431, "Too many headers", "got more than 100 headers")
                return False
            if line == b"\r\n" or line == b"\n" or not line:
                break
            lines.append(codecs.lookup("iso-8859-1").decode(line)[0])
        message = self.MessageClass()
        message._parse(lines)
        self.headers = message
        connection = (self.headers.get("Connection", "") or "").lower()
        if connection == "close":
            self.close_connection = True
        elif connection == "keep-alive" and self.protocol_version.encode() >= b"HTTP/1.1":
            self.close_connection = False
        if ((self.headers.get("Expect", "") or "").lower() == "100-continue"
                and self.protocol_version.encode() >= b"HTTP/1.1" and self.request_version.encode() >= b"HTTP/1.1"):
            return self.handle_expect_100()
        return True

    def handle_expect_100(self) -> bool:
        self.send_response_only(100)
        self.end_headers()
        return True

    def handle_one_request(self) -> None:
        try:
            self.raw_requestline = self.rfile.readline(65537)
            if len(self.raw_requestline) > 65536:
                self.requestline = ""
                self.request_version = ""
                self.command = ""
                self.send_error(414)
                return
            if not self.raw_requestline:
                self.close_connection = True
                return
            if not self.parse_request():
                return
            command = self.command or ""
            try:
                getattr(self, "do_" + command)
            except AttributeError:
                self.send_error(501, "Unsupported method (" + repr(self.command) + ")")
                return
            getattr(self, "do_" + command)()
            self.wfile.flush()
        except TimeoutError as exc:
            self.log_error("Request timed out: %r", exc)
            self.close_connection = True

    def handle(self) -> None:
        self.close_connection = True
        self.handle_one_request()
        while not self.close_connection:
            self.handle_one_request()

    def send_response(self, code: int, message: str | None = None) -> None:
        self.log_request(code)
        self.send_response_only(code, message)
        self.send_header("Server", self.version_string())
        self.send_header("Date", self.date_time_string())

    def send_response_only(self, code: int, message: str | None = None) -> None:
        if self.request_version != "HTTP/0.9":
            if message is None:
                message = self.responses[code][0] if code in self.responses else ""
            if self._buffer is None:
                self._headers_buffer = []
            self._headers_buffer.append((self.protocol_version + " " + str(code) + " " + message + "\r\n").encode("latin-1"))

    def send_header(self, keyword: str, value: str) -> None:
        if self.request_version != "HTTP/0.9":
            if self._buffer is None:
                self._headers_buffer = []
            self._headers_buffer.append((keyword + ": " + value + "\r\n").encode("latin-1"))
        if keyword.lower() == "connection":
            if value.lower() == "close":
                self.close_connection = True
            elif value.lower() == "keep-alive":
                self.close_connection = False

    def end_headers(self) -> None:
        if self.request_version != "HTTP/0.9":
            self._headers_buffer.append(b"\r\n")
            self.flush_headers()

    def flush_headers(self) -> None:
        if self._buffer is not None:
            data = b""
            for chunk in self._headers_buffer:
                data += chunk
            self.wfile.write(data)
            self._headers_buffer = []

    def send_error(self, code: int, message: str | None = None, explain: str | None = None) -> None:
        short, long = self.responses.get(code, ("???", "???"))
        if message is None:
            message = short
        if explain is None:
            explain = long
        self.log_error("code %d, message %s", code, message)
        self.send_response(code, message)
        self.send_header("Connection", "close")
        body = b""
        if code >= 200 and code != 204 and code != 205 and code != 304:
            fields: dict[str, object] = {"code": code, "message": _escape(message), "explain": _escape(explain)}
            content = self.error_message_format % fields
            body = content.encode("utf-8", "replace")
            self.send_header("Content-Type", self.error_content_type)
            self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD" and body:
            self.wfile.write(body)

    def version_string(self) -> str:
        return self.server_version + " " + self.sys_version

    def address_string(self) -> str:
        return self.client_address[0]

    def date_time_string(self, timestamp: float | None = None) -> str:
        stamp = time.time() if timestamp is None else timestamp
        year, month, day, hh, mm, ss, wd, yd, dst = time.gmtime(stamp)
        return "%s, %02d %s %04d %02d:%02d:%02d GMT" % (self.weekdayname[wd], day, str(self.monthname[month]), year, hh, mm, ss)

    def log_date_time_string(self) -> str:
        year, month, day, hh, mm, ss, wd, yd, dst = time.localtime(time.time())
        return "%02d/%s/%04d %02d:%02d:%02d" % (day, str(self.monthname[month]), year, hh, mm, ss)

    def log_request(self, code: int, size: str = "-") -> None:
        self.log_message('"%s" %s %s', self.requestline, str(code), size)

    def log_error(self, format: str, *args: object) -> None:
        self.log_message(format, *args)

    def log_message(self, format: str, *args: object) -> None:
        message = format % args
        safe = ""
        for ch in message:
            n = ord(ch)
            if n in self._control_char_table:
                safe += self._control_char_table[n]
            else:
                safe += ch
        sys.stderr.write(self.address_string() + " - - [" + self.log_date_time_string() + "] " + safe + "\n")
