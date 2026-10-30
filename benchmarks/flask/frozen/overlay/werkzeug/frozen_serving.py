"""Loopback HTTP/1 transport for a frozen application, one request per connection."""

import socket
from flask.frozen.application import Application
from werkzeug.wrappers.frozen import Request


def read_request(conn: socket.socket) -> Request:
    packet = b""
    end = -1
    while end < 0:
        chunk = conn.recv(4096)
        if not chunk:
            raise ValueError("incomplete headers")
        before = len(packet)
        packet += chunk
        for i in range(max(0, before - 3), len(packet) - 3):
            if packet[i] == 13 and packet[i + 1] == 10 and packet[i + 2] == 13 and packet[i + 3] == 10:
                end = i
                break
        if end > 8192 or (end < 0 and len(packet) > 8192):
            raise ValueError("headers too large")
    headers = packet[:end].decode("ascii")
    lines = headers.split("\r\n")
    words = lines[0].split(" ")
    if len(words) != 3 or not words[1].startswith("/") or words[2] not in ("HTTP/1.0", "HTTP/1.1"):
        raise ValueError("invalid request line")
    method = words[0]
    target = words[1]
    length = 0
    media = ""
    cookie = ""
    has_length = False
    for i in range(1, len(lines)):
        name, sep, text = lines[i].partition(":")
        if not sep or not name or name.strip() != name:
            raise ValueError("invalid header")
        key = name.lower()
        value = text.strip()
        if key == "transfer-encoding":
            raise ValueError("chunked requests unavailable")
        if key == "content-type":
            media = value.partition(";")[0].lower()
        if key == "cookie":
            cookie = (cookie + "; " + value) if cookie else value
        if key == "content-length":
            if has_length or not value or len(value) > 9:
                raise ValueError("invalid Content-Length")
            for char in value:
                if ord(char) < 48 or ord(char) > 57:
                    raise ValueError("invalid Content-Length")
            has_length = True
            length = int(value)
            if length > 1048576:
                raise ValueError("body too large")
    body = packet[end + 4:]
    while len(body) < length:
        chunk = conn.recv(min(length - len(body), 16384))
        if not chunk:
            raise ValueError("incomplete body")
        body += chunk
    return Request(method, target, media, cookie, body[:length])


def serve(app: Application, port_file: str) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    with open(port_file, "w") as ready:
        ready.write(str(listener.getsockname()[1]))
    while True:
        conn, _ = listener.accept()
        try:
            head = False
            try:
                req = read_request(conn)
                head = req.method == "HEAD"
                out = app.dispatch(req)
            except ValueError:
                out = app.error(400)
            conn.sendall(out.wire(app.reasons.get(out.status, "Unknown"), head))
        except OSError:
            pass
        finally:
            conn.close()
