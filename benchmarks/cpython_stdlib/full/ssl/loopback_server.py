"""Blocking SSLContext server for the CPython test_ssl loopback fixture."""
import errno
import socket
import ssl
import sys


class ClosingBacklog:
    def __init__(self, sock):
        self.sock = sock

    def __index__(self):
        self.sock.close()
        return 1


class ShutdownWrite:
    def __init__(self, sock):
        self.sock = sock

    def __index__(self):
        # SSLSocket discards its TLS state before the underlying socket converts how.
        assert self.sock.get_channel_binding() is None
        assert self.sock.recv(1) == b"R"
        self.sock.sendall(memoryview(b"S"))
        return socket.SHUT_WR


def accept_one(listener):
    listener.listen(1)
    print("PORT", listener.getsockname()[1], flush=True)
    return listener.accept()


def main():
    mode, certfile, keyfile = sys.argv[1:]
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    assert not ctx.check_hostname and ctx.verify_mode == ssl.CERT_NONE
    try:
        ctx.load_cert_chain(certfile, None if keyfile == "-" else keyfile)
    except ssl.SSLError as exc:
        if mode != "invalid":
            raise
        print("LOAD_ERROR", str(exc), flush=True)
        return
    if mode == "load":
        print("LOADED", flush=True)
        return
    assert mode in ("serve", "reject", "listen", "shutdown", "unwrap", "ragged")
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        if mode == "listen":
            with ctx.wrap_socket(listener, server_side=True) as tls_listener:
                assert listener.fileno() == -1
                wrapped, address = accept_one(tls_listener)
                assert wrapped.getpeername() == address
                assert wrapped.getsockname() == tls_listener.getsockname()
        else:
            raw, _ = accept_one(listener)
            with raw:
                try:
                    wrapped = ctx.wrap_socket(
                        raw, server_side=True, suppress_ragged_eofs=mode not in ("unwrap", "ragged")
                    )
                except ssl.SSLError as exc:
                    assert mode == "reject" and raw.fileno() == -1
                    print("PEER_REJECTED", str(exc), flush=True)
                    return
                assert mode in ("serve", "shutdown", "unwrap", "ragged") and raw.fileno() == -1
        with wrapped as peer:
            assert peer.server_side and peer.server_hostname is None and peer.context is ctx
            assert peer.get_verified_chain() == []
            assert peer.get_unverified_chain() == []
            binding = peer.get_channel_binding()
            assert isinstance(binding, bytes) and binding
            print("BINDING", binding.hex(), flush=True)
            data = b""
            if mode in ("unwrap", "ragged"):
                data = peer.read(1)
                assert data == b"L" and peer.pending() == 9
                buf = bytearray(b"-----")
                assert peer.read(3, memoryview(buf)[1:4]) == 3
                assert buf == b"-OCA-" and peer.pending() == 6
                data += buf[1:4]
            while len(data) < 10:
                chunk = peer.recv(10 - len(data))
                assert chunk
                data += chunk
            assert data == b"LOCAL TLS\n"
            peer.sendall(data.decode("ascii").lower().encode("ascii"))
            if mode == "unwrap":
                fd = peer.fileno()
                assert peer.pending() == 0 and peer.read(1) == b""
                assert peer.unwrap() is peer and peer.fileno() == fd
                assert peer.pending() == 0 and peer.recv(1) == b"R"
                peer.sendall(b"S")
            if mode == "ragged":
                assert peer.pending() == 0
                try:
                    peer.read(1)
                except ssl.SSLEOFError as exc:
                    assert exc.errno == ssl.SSL_ERROR_EOF
                else:
                    raise AssertionError("transport EOF accepted as close_notify")
            if mode == "shutdown":
                fd = peer.fileno()
                with peer.makefile("rb", buffering=0) as reader:
                    peer.shutdown(ShutdownWrite(peer))
                    peer.close()
                    assert peer.fileno() == fd
                    assert reader.read(1) == b"T"
                    assert reader.read(1) == b""
                assert peer.fileno() == -1
            if mode == "listen":
                try:
                    peer.listen(ClosingBacklog(peer))
                except OSError as exc:
                    assert exc.errno == errno.EBADF
                else:
                    raise AssertionError("listen succeeded after close")


main()
