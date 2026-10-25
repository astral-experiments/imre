import errno
import os
import socket


def last_owner() -> int:
    sock = socket.socket()
    return sock.fileno()


def externally_closed() -> None:
    sock = socket.socket()
    os.close(sock.fileno())
    # Silent finalization must not abort on the stale public descriptor.


def accept_into(listener: socket.socket, clients: dict[int, socket.socket]) -> tuple[int, tuple[str, int]]:
    conn, addr = listener.accept()
    fd = conn.fileno()
    clients[fd] = conn
    return fd, addr


def duplicated_connection() -> None:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        with socket.socket() as original:
            original.connect(listener.getsockname())
            peer, _ = listener.accept()
            with peer:
                original.set_inheritable(inheritable=True)
                assert os.get_inheritable(original.fileno())
                try:
                    original.set_inheritable(1 << 80)
                except OverflowError:
                    assert original.get_inheritable()
                else:
                    raise AssertionError("accepted an overflowing descriptor flag")
                original.setblocking(False)
                # dup reapplies the Python socket's mode even after a raw fd change.
                os.set_blocking(original.fileno(), True)
                with original.dup() as copy:
                    assert copy.fileno() != original.fileno()
                    assert not copy.get_inheritable() and original.get_inheritable()
                    assert not copy.getblocking() and not os.get_blocking(original.fileno())
                    os.set_inheritable(copy.fileno(), True)
                    assert copy.get_inheritable()
                    copy.set_inheritable(False)
                    assert not os.get_inheritable(copy.fileno())
                    original.close()
                    try:
                        original.dup()
                    except OSError as exc:
                        assert exc.errno == errno.EBADF
                    else:
                        raise AssertionError("duplicated a closed socket")
                    copy.sendall(b"!")
                    assert peer.recv(1) == b"!"
                try:
                    copy.get_inheritable()
                except OSError as exc:
                    assert exc.errno == errno.EBADF
                else:
                    raise AssertionError("read cached flags after close")
                try:
                    copy.set_inheritable(True)
                except OSError as exc:
                    assert exc.errno == errno.EBADF
                else:
                    raise AssertionError("changed flags on a closed socket")


def main() -> None:
    duplicated_connection()
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        assert listener.getsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR) != 0
        listener.bind(("127.0.0.1", 0))
        address = listener.getsockname()
        # Like CPython's test_connect, probe a released port. Darwin can silently
        # drop a SYN to a socket that is still bound but has never listened.
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            closed_address = reservation.getsockname()
        with socket.socket() as refused:
            try:
                refused.connect(closed_address)
            except ConnectionRefusedError as e:
                assert e.errno == errno.ECONNREFUSED
            else:
                raise AssertionError("connection unexpectedly succeeded")
        listener.listen()
        listener.setblocking(False)
        try:
            listener.accept()
        except Warning:
            raise AssertionError("socket error was caught as a warning")
        except BlockingIOError as e:
            assert e.errno == errno.EAGAIN or e.errno == errno.EWOULDBLOCK
        else:
            raise AssertionError("empty listener accepted a connection")
        try:
            raise RuntimeWarning("peer warning")
        except OSError:
            raise AssertionError("warning was caught as a socket error")
        except Warning as warning:
            assert str(warning) == "peer warning"
        listener.setblocking(True)
        with socket.socket() as client:
            client.connect(address)
            clients: dict[int, socket.socket] = {}
            fd, peer = accept_into(listener, clients)
            server = clients[fd]
            with server:
                sockets = {client: 7, server: 9}
                assert sockets[client] == 7 and sockets[server] == 9
                assert server is not listener
                assert peer == client.getsockname()
                assert not os.get_inheritable(server.fileno())
                assert not os.get_inheritable(client.fileno())
                server.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 0)
                disabled = server.getsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY)
                server.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                enabled = server.getsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY)
                # Darwin returns the kernel flag mask when enabled.
                assert disabled == 0 and enabled != 0
                print(f"TCP_NODELAY {disabled} {enabled}")
                client.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
                server.setblocking(False)
                assert not clients[fd].getblocking()
                del clients[fd]
                assert server.fileno() == fd
                assert not os.get_blocking(server.fileno())
                try:
                    server.recv(1)
                except BlockingIOError:
                    pass
                else:
                    raise AssertionError("would-block reported EOF")
                assert client.send(b"abc") == 3
                # A successful peer send can precede delivery on loopback.
                # Wait for the payload and allow TCP to split it across reads.
                server.setblocking(True)
                assert server.recv(1, socket.MSG_PEEK) == b"a"
                first = server.recv(1)
                tail = b""
                while len(tail) < 2:
                    part = server.recv(4)
                    assert len(part) > 0
                    tail += part
                assert tail == b"bc"
                assert first == b"a"
                assert client.send(b"") == 0
                # This peer deliberately does not read. Exact counts vary by kernel.
                server.setblocking(False)
                server.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
                block = b"x"
                for i in range(22):
                    block += block
                n = server.send(block)
                assert 0 < n and n < len(block)
                for i in range(128):
                    try:
                        server.send(block)
                    except BlockingIOError:
                        break
                else:
                    raise AssertionError("send buffer never filled")
                try:
                    server.sendall(block)
                except BlockingIOError:
                    pass
                else:
                    raise AssertionError("sendall silently dropped a suffix")
    alias = listener
    assert alias.fileno() == -1
    assert alias.recv(0) == b""
    try:
        alias.sendall(b"")
    except OSError as e:
        assert e.errno == errno.EBADF
    else:
        raise AssertionError("closed empty send succeeded")
    try:
        alias.recv(-1)
    except ValueError as e:
        assert str(e) == "negative buffersize in recv"
    else:
        raise AssertionError("negative recv size accepted")
    with socket.socket() as replacement:
        alias.close()
        replacement.bind(("127.0.0.1", 0))
    externally_closed()
    stale = socket.socket()
    os.close(stale.fileno())
    try:
        stale.close()
    except OSError as e:
        assert e.errno == errno.EBADF and stale.fileno() == -1
    else:
        raise AssertionError("explicit close suppressed EBADF")
    with socket.socket() as invalid:
        try:
            invalid.bind(("127.0.0.1", 1 << 48))
        except OverflowError as e:
            assert str(e) == "bind(): port must be 0-65535."
        else:
            raise AssertionError("port overflow accepted")
        try:
            invalid.connect(("::1", 0))
        except socket.gaierror as e:
            assert e.errno is not None
        else:
            raise AssertionError("IPv6 literal resolved to IPv4")
    try:
        os.get_inheritable(last_owner())
    except OSError as e:
        assert e.errno == errno.EBADF
    else:
        raise AssertionError("last socket reference leaked its descriptor")
    print("socket operations passed")


main()
