"""Idiomatic CPython socket/bytes implementation for many persistent clients."""

import selectors
import socket
import sys


def handle(c, buffers):
    part = c.recv(4096)
    if not part:
        return False
    buf = buffers[c] + part
    while True:
        end = buf.find(b"\r\n\r\n")
        if end > 8192 or (end < 0 and len(buf) > 8192):
            c.sendall(
                b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            return False
        if end < 0:
            buffers[c] = buf
            return True
        head, buf = buf[: end + 4], buf[end + 4 :]
        if head.startswith(b"GET /hello HTTP/1.1\r\n"):
            prefix = (
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n"
            )
            body = b"hello\n"
        elif head.startswith(b"GET "):
            prefix = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n"
            body = b"not found\n"
        else:
            c.sendall(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            return False
        close = b"\r\nconnection: close\r\n" in head.lower()
        c.sendall(
            prefix
            + (
                b"Connection: close\r\n\r\n"
                if close
                else b"Connection: keep-alive\r\n\r\n"
            )
            + body
        )
        if close:
            return False


with selectors.DefaultSelector() as sel, socket.socket() as listener:
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", int(sys.argv[1])))
    listener.listen(128)
    sel.register(listener, selectors.EVENT_READ)
    buffers = {}
    while True:
        for key, events in sel.select():
            c = key.fileobj
            if c is listener:
                conn, _ = listener.accept()
                conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                buffers[conn] = b""
                sel.register(conn, selectors.EVENT_READ)
            else:
                try:
                    keep = handle(c, buffers)
                except (BrokenPipeError, ConnectionResetError):
                    keep = False
                if not keep:
                    sel.unregister(c)
                    del buffers[c]
                    c.close()
