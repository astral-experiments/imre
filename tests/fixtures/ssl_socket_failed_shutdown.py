import errno
import socket
import ssl
import sys


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

# A closed transport half must not discard authenticated data or expose raw bytes.
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
        assert server.pending() == 49 and server.recv(64) == b"b" * 49
        assert server.recv(1) == b""
