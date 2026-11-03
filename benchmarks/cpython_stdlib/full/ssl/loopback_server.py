"""Blocking SSLContext server for the CPython test_ssl loopback fixture."""
import socket
import ssl
import sys


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
    assert mode in ("serve", "reject")
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        print("PORT", listener.getsockname()[1], flush=True)
        raw, _ = listener.accept()
        with raw:
            try:
                wrapped = ctx.wrap_socket(raw, server_side=True)
            except ssl.SSLError as exc:
                assert mode == "reject" and raw.fileno() == -1
                print("PEER_REJECTED", str(exc), flush=True)
                return
            with wrapped as peer:
                assert mode == "serve" and raw.fileno() == -1
                assert peer.server_side and peer.server_hostname is None and peer.context is ctx
                assert peer.get_verified_chain() == []
                assert peer.get_unverified_chain() == []
                binding = peer.get_channel_binding()
                assert isinstance(binding, bytes) and binding
                print("BINDING", binding.hex(), flush=True)
                data = b""
                while len(data) < 10:
                    chunk = peer.recv(10 - len(data))
                    assert chunk
                    data += chunk
                assert data == b"LOCAL TLS\n"
                peer.sendall(data.decode("ascii").lower().encode("ascii"))


main()
