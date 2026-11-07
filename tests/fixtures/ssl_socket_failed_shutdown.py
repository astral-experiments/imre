import errno
import socket
import ssl


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
