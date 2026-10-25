"""Small HTTP/1.1 GET endpoint using only public Python socket and poll APIs."""

import select
import socket
import sys


def end_headers(buf: bytes) -> int:
    for i in range(len(buf) - 3):
        if buf[i] == 13 and buf[i + 1] == 10 and buf[i + 2] == 13 and buf[i + 3] == 10:
            return i
    return -1


def close_requested(buf: bytes) -> bool:
    token = b"\r\nconnection: close\r\n"
    for i in range(len(buf) - len(token) + 1):
        if buf[i] != 13:
            continue
        j = 0
        while j < len(token):
            v = buf[i + j]
            if 65 <= v <= 90:
                v += 32
            if v != token[j]:
                break
            j += 1
        if j == len(token):
            return True
    return False


def response(head: bytes) -> tuple[bytes, bool]:
    close = close_requested(head)
    if head[:21] == b"GET /hello HTTP/1.1\r\n":
        if close:
            return (
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: close\r\n\r\nhello\n",
                True,
            )
        return (
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: keep-alive\r\n\r\nhello\n",
            False,
        )
    if head[:4] == b"GET ":
        if close:
            return (
                b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 10\r\nConnection: close\r\n\r\nnot found\n",
                True,
            )
        return (
            b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 10\r\nConnection: keep-alive\r\n\r\nnot found\n",
            False,
        )
    return (
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        True,
    )


def parse(buf: bytes, pending: bytes) -> tuple[bytes, bytes, bool]:
    end = end_headers(buf)
    while 0 <= end <= 8192:
        reply, close = response(buf[: end + 4])
        pending = pending + reply
        if close:
            return b"", pending, True
        buf = buf[end + 4 :]
        end = end_headers(buf)
    if end > 8192 or len(buf) > 8192:
        return (
            b"",
            pending
            + b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            True,
        )
    return buf, pending, False


def main() -> None:
    port = int(sys.argv[-1])
    sndbuf = int(sys.argv[1]) if len(sys.argv) == 3 else 0
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
                            buf, out, done = parse(buf + chunk, out)
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


main()
