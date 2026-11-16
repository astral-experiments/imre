import errno
import os
import socket
import ssl
import sys


# Listener I/O guards do not consult overridable connection queries.
queries = []

class Listener(ssl.SSLSocket):
    def _check_connected(self):
        queries.append("_check_connected")
        return super()._check_connected()

    def getpeername(self):
        queries.append("getpeername")
        return super().getpeername()


listening = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
listening.sslsocket_class = Listener
with socket.socket() as raw:
    raw.bind(("127.0.0.1", 0))
    raw.listen()
    with listening.wrap_socket(raw, server_side=True) as listener:
        queries.clear()
        for operation, args in ((listener.sendto, (b"x", listener.getsockname())),
                                (listener.recvfrom, (1,)), (listener.recvfrom_into, (bytearray(1), 1)),
                                (listener.recv_into, (bytearray(1),))):
            try:
                operation(*args)
            except ValueError:
                pass
            else:
                raise AssertionError("unguarded listening socket")
        assert not queries


context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
for how in (77, "invalid"):
    raw, remote = socket.socketpair()
    with raw, remote, context.wrap_socket(
        raw, server_hostname="localhost", do_handshake_on_connect=False
    ) as peer:
        peer.setblocking(False)
        try:
            peer.shutdown(how)
        except OSError as exc:
            assert how == 77 and exc.errno == errno.EINVAL
        except TypeError:
            assert how == "invalid"
        else:
            raise AssertionError("invalid shutdown succeeded")

        try:
            peer.sendall(b"secret after failed shutdown")
        except ssl.SSLWantReadError:
            pass
        else:
            raise AssertionError("wrote without authenticating the peer")
        hello = remote.recv(16384)
        assert hello[:1] == b"\x16" and b"secret after failed shutdown" not in hello

# Keep the open direction authenticated until the transport is shut in both directions.
certs = sys.argv[1]
context.load_verify_locations(certs + "/ssl_cert.pem")
server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_context.load_cert_chain(certs + "/keycert.pem")
left, right = socket.socketpair()
left.setblocking(False)
right.setblocking(False)
with context.wrap_socket(left, server_hostname="localhost", do_handshake_on_connect=False) as client:
    with server_context.wrap_socket(right, server_side=True, do_handshake_on_connect=False) as server:
        waiting = [client, server]
        for _ in range(20):
            for sock in waiting[:]:
                try:
                    sock.do_handshake()
                except (ssl.SSLWantReadError, ssl.SSLWantWriteError):
                    continue
                waiting.remove(sock)
            if not waiting:
                break
        assert not waiting
        # A between-operation dup2 must not redirect the socket BIO.
        donor, unrelated = socket.socketpair()
        with donor, unrelated:
            visible = client.fileno()
            original = os.dup(visible)
            try:
                class ReplaceFlags:
                    def __ne__(self, zero):
                        assert zero == 0
                        os.dup2(donor.fileno(), visible)
                        client.sendall(b"nested")
                        return False

                    def __bool__(self):
                        raise AssertionError("TLS flags must be compared to zero")

                client.sendall(b"outer", ReplaceFlags())
                assert server.recv(6) == b"nested"
                assert server.recv(5) == b"outer"
                client.sendall(b"original")
                received = server.recv(8)
                assert received == b"original"
                unrelated.setblocking(False)
                try:
                    unrelated.recv(1)
                except BlockingIOError:
                    pass
                else:
                    raise AssertionError("unrelated socket received TLS bytes")
                assert os.fstat(client.fileno()).st_ino == os.fstat(original).st_ino
            finally:
                if client.fileno() == visible:
                    os.dup2(original, visible)
                else:
                    os.close(visible)
                os.close(original)
        client.shutdown(socket.SHUT_RD)
        assert client.recv(1) == b""
        server.shutdown(socket.SHUT_WR)
        try:
            server.sendall(b"closed")
        except BrokenPipeError:
            pass
        else:
            raise AssertionError("wrote to a closed write half")
        client.sendall(b"a" + b"b" * 49)
        assert server.recv(1) == b"a" and server.pending() == 49
        server.shutdown(socket.SHUT_RDWR)
        assert server.pending() == 0 and server.recv(64) == b""
        data = bytearray(b"x" * 49)
        assert server.recv_into(data, flags=socket.MSG_PEEK) == 0
        assert data == b"x" * 49
        if sys.platform == "linux":
            try:
                server.recv(1, socket.MSG_OOB)
            except OSError as exc:
                assert exc.errno == errno.EINVAL
            else:
                raise AssertionError("accepted an urgent read on a shut Unix stream")
        # Closing a TLS socket must leave the external replacement with its owner.
        donor, unrelated = socket.socketpair()
        with donor, unrelated:
            visible = server.fileno()
            os.dup2(donor.fileno(), visible)
            try:
                server.close()
                os.write(visible, b"owned")
                received = unrelated.recv(5)
                assert received == b"owned"
            finally:
                os.close(visible)
