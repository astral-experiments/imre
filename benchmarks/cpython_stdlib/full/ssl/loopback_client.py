"""Secure SSLContext client for the CPython test_ssl loopback fixture."""
import socket
import ssl
import sys


def main():
    host, port, cafile, hostname, expected = sys.argv[1:]
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    assert ctx.check_hostname and ctx.verify_mode == ssl.CERT_REQUIRED
    ctx.load_verify_locations(cafile=cafile)
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as raw:
        raw.connect((host, int(port)))
        try:
            peer = ctx.wrap_socket(raw, server_hostname=hostname)
        except ssl.SSLCertVerificationError as exc:
            assert raw.fileno() == -1
            assert int(expected) != 0 and exc.verify_code == int(expected)
            assert exc.verify_message
            print("VERIFY_ERROR", exc.verify_code, exc.verify_message)
            return
        with peer:
            assert raw.fileno() == -1
            assert int(expected) == 0
            peer.sendall(b"LOCAL TLS\n")
            data = b""
            while len(data) < 10:
                chunk = peer.recv(10 - len(data))
                assert chunk
                data += chunk
            assert data == b"local tls\n"
            assert peer.recv(0) == b""
            peer.sendall(b"over\n")
            print("VERIFIED_ECHO", hostname)
        peer.close()


main()
