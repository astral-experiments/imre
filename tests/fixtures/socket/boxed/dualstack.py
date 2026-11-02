"""Exercise the public probe with real, freshly opened IPv6 sockets."""
import socket
from socket import has_dualstack_ipv6


def check():
    assert has_dualstack_ipv6 is socket.has_dualstack_ipv6
    try:
        with socket.socket(socket.AF_INET6, socket.SOCK_STREAM) as probe:
            probe.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
    except OSError:
        expected = False
    else:
        expected = True
    assert has_dualstack_ipv6() is expected
    if not expected:
        return

    opened = []
    constructor = socket.socket
    option = socket.IPV6_V6ONLY
    supported = socket.has_ipv6

    def track(family, kind):
        sock = constructor(family, kind)
        opened.append(sock)
        return sock

    try:
        socket.socket = track
        socket.has_ipv6 = False
        assert not has_dualstack_ipv6() and not opened
        socket.has_ipv6 = supported
        assert has_dualstack_ipv6()
        socket.IPV6_V6ONLY = -1  # Have the kernel reject this setsockopt.
        assert not has_dualstack_ipv6()
        assert len(opened) == 2
        assert opened[0].fileno() == opened[1].fileno() == -1
    finally:
        socket.socket = constructor
        socket.has_ipv6 = supported
        socket.IPV6_V6ONLY = option


def check_live_capabilities():
    import _socket

    assert socket._socket is _socket
    assert socket.error is _socket.error is OSError
    raw = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    try:
        assert raw.family == _socket.AF_INET
        assert raw.fileno() >= 0
    finally:
        raw.close()
    assert raw.fileno() == -1
    original = socket.socket

    def unexpected(*args):
        raise AssertionError("socket opened before a missing capability check")

    def exhausted(*args):
        raise OSError("descriptor limit")

    def invalid(*args):
        raise ValueError("constructor failed")

    try:
        socket.socket = unexpected
        for name in ("IPPROTO_IPV6", "IPV6_V6ONLY"):
            value = getattr(_socket, name)
            delattr(_socket, name)
            try:
                assert has_dualstack_ipv6() is False
            finally:
                setattr(_socket, name, value)
        socket.socket = exhausted
        assert has_dualstack_ipv6() is False
        socket.socket = invalid
        try:
            has_dualstack_ipv6()
        except ValueError as exc:
            assert str(exc) == "constructor failed"
        else:
            raise AssertionError("constructor ValueError was suppressed")
    finally:
        socket.socket = original


check()
check_live_capabilities()
