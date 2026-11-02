"""Exercise runtime-imported IPv6 stream endpoints with a local client and server."""
import errno
import socket


def check_stream():
    try:
        listener = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    except OSError as exc:
        if exc.errno in (errno.EAFNOSUPPORT, errno.EPROTONOSUPPORT):
            return
        raise
    with listener:
        assert listener.family == socket.AF_INET6
        assert listener.getsockname() == ("::", 0, 0, 0)
        try:
            listener.bind(("::1", 0, 0, 0))
        except OSError as exc:
            if exc.errno == errno.EADDRNOTAVAIL:
                return
            raise
        host, port, flow, scope = listener.getsockname()
        assert (host, flow, scope) == ("::1", 0, 0) and port > 0
        rows = socket.getaddrinfo(host, port, socket.AF_INET6, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
        assert rows == [(socket.AF_INET6, socket.SOCK_DGRAM, socket.IPPROTO_UDP, "", (host, port, 0, 0))]
        listener.listen(1)
        with socket.socket(socket.AF_INET6) as client:
            client.connect((host, port))
            server, peer = listener.accept()
            with server:
                assert server.family == socket.AF_INET6
                assert server.getsockname() == (host, port, flow, scope)
                assert peer == client.getsockname()
                client.sendall(b"6")
                assert server.recv(1) == b"6"
    assert listener.fileno() == -1 and listener.family == socket.AF_INET6
    try:
        listener.getsockname()
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("closed IPv6 socket returned an address")


def check_arguments():
    with socket.socket(socket.AF_INET6) as probe:
        for addr in (("::1",), ("::1", 0, 0, 0, 1)):
            try:
                probe.bind(addr)
            except TypeError as exc:
                assert str(exc) == "AF_INET6 address must be a tuple (host, port[, flowinfo[, scopeid]])"
            else:
                raise AssertionError("invalid IPv6 address tuple was accepted")
        for addr, message in ((("::1", 65536), "bind(): port must be 0-65535."),
                              (("::1", 0, 0x100000), "bind(): flowinfo must be 0-1048575.")):
            try:
                probe.bind(addr)
            except OverflowError as exc:
                assert str(exc) == message
            else:
                raise AssertionError("out-of-range IPv6 address field was accepted")
        probe.bind(("::", 0, 0))
        assert probe.getsockname()[0] == "::"


check_stream()
try:
    check_arguments()
except OSError as exc:
    if exc.errno not in (errno.EAFNOSUPPORT, errno.EPROTONOSUPPORT):
        raise
