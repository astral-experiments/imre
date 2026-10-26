"""Bounded HTTP/1.1 transport for the local task board."""

import select
import socket
from tinyweb import Request, Response, Router, error


def header_end(buf: bytes) -> int:
    for i in range(len(buf) - 3):
        if buf[i] == 13 and buf[i + 1] == 10 and buf[i + 2] == 13 and buf[i + 3] == 10:
            return i
    return -1


def reason(status: int) -> str:
    if status == 200:
        return "OK"
    if status == 201:
        return "Created"
    if status == 400:
        return "Bad Request"
    if status == 404:
        return "Not Found"
    if status == 405:
        return "Method Not Allowed"
    if status == 413:
        return "Content Too Large"
    if status == 415:
        return "Unsupported Media Type"
    if status == 417:
        return "Expectation Failed"
    if status == 431:
        return "Request Header Fields Too Large"
    return "Internal Server Error"


def wire(res: Response, close: bool, head_only: bool) -> bytes:
    conn = "close" if close else "keep-alive"
    header = (
        "HTTP/1.1 "
        + str(res.status)
        + " "
        + reason(res.status)
        + "\r\nContent-Type: "
        + res.media
        + "\r\nContent-Length: "
        + str(len(res.body))
        + "\r\nConnection: "
        + conn
        + "\r\n"
    )
    if res.allow:
        header += "Allow: " + res.allow + "\r\n"
    prefix = (header + "\r\n").encode("utf-8")
    if head_only:
        return prefix
    return prefix + res.body


def fields(raw: bytes) -> tuple[str, str, str, int, bool, int]:
    lines = raw.decode("ascii").split("\r\n")
    a = lines[0].split(" ")
    if len(a) != 3 or not a[0] or not a[1].startswith("/") or a[2] != "HTTP/1.1":
        raise ValueError("bad request line")
    path = a[1].split("?")[0]
    for c in a[1]:
        if ord(c) < 33 or ord(c) == 127:
            raise ValueError("invalid request target")
    length, seen_length, host, close, media, problem = 0, False, False, False, "", 0
    for i in range(1, len(lines)):
        line = lines[i]
        pos = line.find(":")
        if pos <= 0:
            raise ValueError("invalid header")
        key = line[:pos].lower()
        for c in key:
            if ord(c) <= 32 or ord(c) == 127:
                raise ValueError("invalid header name")
        value = line[pos + 1 :].strip()
        if key == "host":
            if host or not value:
                raise ValueError("bad Host")
            host = True
        elif key == "content-length":
            if seen_length or not value:
                raise ValueError("bad Content-Length")
            seen_length = True
            for c in value:
                if ord(c) < 48 or ord(c) > 57:
                    raise ValueError("invalid Content-Length")
                if length <= 16384:
                    length = length * 10 + ord(c) - 48
            if length > 16384:
                problem = 413
        elif key == "transfer-encoding":
            raise ValueError("transfer coding unavailable")
        elif key == "expect":
            problem = 417
        elif key == "content-type":
            media = value.split(";")[0].strip().lower()
        elif key == "connection":
            for token in value.lower().split(","):
                if token.strip() == "close":
                    close = True
    if not host:
        raise ValueError("missing Host")
    return a[0], path, media, length, close, problem


def consume(app: Router, buf: bytes, pending: bytes) -> tuple[bytes, bytes, bool]:
    while len(buf):
        end = header_end(buf)
        if end > 8192 or (end < 0 and len(buf) > 8192):
            return (
                b"",
                pending + wire(error(431, "headers too large"), True, False),
                True,
            )
        if end < 0:
            break
        try:
            method, path, media, length, close, problem = fields(buf[:end])
        except ValueError:
            return b"", pending + wire(error(400, "bad request"), True, False), True
        if problem:
            return (
                b"",
                pending + wire(error(problem, reason(problem)), True, False),
                True,
            )
        finish = end + 4 + length
        if len(buf) < finish:
            break
        body = buf[end + 4 : finish]
        buf = buf[finish:]
        head_only = method == "HEAD"
        req = Request("GET" if head_only else method, path, body, media)
        try:
            ans = app.dispatch(req)
        except OSError:
            ans = error(500, "storage unavailable")
            close = True
        pending += wire(ans, close, head_only)
        if close:
            return b"", pending, True
    return buf, pending, False


def serve(app: Router, port: int, sndbuf: int = 0) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", port))
    listener.listen(128)
    listener.setblocking(False)
    p = select.poll()
    lfd = listener.fileno()
    p.register(lfd, select.POLLIN)
    clients: dict[int, socket.socket] = {}
    inputs: dict[int, bytes] = {}
    outputs: dict[int, bytes] = {}
    closing: dict[int, bool] = {}
    interests: dict[int, int] = {}
    while True:
        for fd, mask in p.poll():
            if fd == lfd:
                try:
                    conn, _ = listener.accept()
                except BlockingIOError:
                    continue
                conn.setblocking(False)
                conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                if sndbuf:
                    conn.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, sndbuf)
                nfd = conn.fileno()
                clients[nfd] = conn
                inputs[nfd] = b""
                outputs[nfd] = b""
                closing[nfd] = False
                interests[nfd] = select.POLLIN
                p.register(nfd, select.POLLIN)
                continue
            client = clients[fd]
            buf = inputs[fd]
            out = outputs[fd]
            done = closing[fd]
            try:
                if (
                    not done
                    and len(out) < 65536
                    and mask & (select.POLLIN | select.POLLHUP | select.POLLERR)
                ):
                    try:
                        chunk = client.recv(4096)
                    except BlockingIOError:
                        pass
                    else:
                        if len(chunk) == 0:
                            done = True
                        else:
                            buf, out, done = consume(app, buf + chunk, out)
                if len(out):
                    try:
                        sent = client.send(out)
                    except BlockingIOError:
                        pass
                    else:
                        if sent == 0:
                            done = True
                            out = b""
                        elif sent == len(out):
                            out = b""
                        else:
                            out = out[sent:]
            except OSError:
                done = True
                out = b""
            if done and not len(out):
                p.unregister(fd)
                client.close()
                del clients[fd]
                del inputs[fd]
                del outputs[fd]
                del closing[fd]
                del interests[fd]
            else:
                inputs[fd] = buf
                outputs[fd] = out
                closing[fd] = done
                want = select.POLLIN if not done and len(out) < 65536 else 0
                if len(out):
                    want |= select.POLLOUT
                if want != interests[fd]:
                    p.modify(fd, want)
                    interests[fd] = want
