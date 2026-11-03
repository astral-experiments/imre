"""Secure SSLContext client for the CPython test_ssl loopback fixture."""
import socket
import ssl
import sys


def main():
    host, port, cafile, hostname, expected = sys.argv[1:6]
    leaf_file, wire_count = sys.argv[6:8]
    mode = sys.argv[8:]
    if mode == ["https"]:
        import http.client
        import urllib.request
        with urllib.request.urlopen("data:,local") as response:
            assert response.read() == b"local"
        ctx = http.client._create_https_context(11)
        assert ctx.post_handshake_auth
        required = ssl.VERIFY_X509_STRICT | ssl.VERIFY_X509_PARTIAL_CHAIN
        assert ctx.verify_flags & required == required
    else:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    if mode and mode != ["https"]:
        version = ssl.TLSVersion[mode[0]]
        ctx.minimum_version = version
        ctx.maximum_version = version
        assert ctx.minimum_version == ctx.maximum_version == version
        options = ctx.options
        ctx.options |= ssl.OP_NO_TICKET
        assert ctx.options & ssl.OP_NO_TICKET
        ctx.options = options
        assert ctx.options == options
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
            with open(leaf_file, encoding="ascii") as cert, open(cafile, encoding="ascii") as ca:
                leaf = ssl.PEM_cert_to_DER_cert(cert.read())
                root = ssl.PEM_cert_to_DER_cert(ca.read())
            assert ssl.PEM_cert_to_DER_cert(ssl.DER_cert_to_PEM_cert(leaf)) == leaf
            assert peer.get_verified_chain() == [leaf, root]
            assert peer.get_unverified_chain() == [leaf, root][:int(wire_count)]
            peer.sendall(b"LOCAL TLS\n")
            data = b""
            while len(data) < 10:
                chunk = peer.recv(10 - len(data))
                assert chunk
                data += chunk
            assert data == b"local tls\n"
            assert peer.recv(0) == b""
            assert peer.send(b"") == 0
            binding = peer.get_channel_binding()
            assert isinstance(binding, bytes) and binding
            assert peer.get_channel_binding(cb_type="tls-unique") == binding
            peer.sendall(b"CB tls-unique\n")
            response = b""
            while not response.endswith(b"\n"):
                chunk = peer.recv(1024)
                assert chunk
                response += chunk
            assert response.strip() == repr(binding).encode("ascii")
            with peer.makefile("rb") as reader:
                with peer.makefile("rb", buffering=0) as raw_reader:
                    assert peer.send(memoryview(b"X\n")) == 2
                    buf = bytearray(b"??")
                    assert raw_reader.readinto(memoryview(buf)[1:]) == 1
                    assert buf == b"?x" and raw_reader.read(1) == b"\n"
                peer.sendall(b"LAST LINE\n")
                peer.close()
                assert reader.read(2) == b"la"
                assert reader.readline() == b"st line\n"
                peer.sendall(b"over\n")
                assert reader.read() == b""
            assert reader.closed
            print("VERIFIED_ECHO", hostname)
        peer.close()
        assert peer.get_channel_binding() is None
        for name in ("get_verified_chain", "get_unverified_chain"):
            try:
                getattr(peer, name)()
            except AttributeError as exc:
                assert str(exc) == "'NoneType' object has no attribute '%s'" % name
            else:
                raise AssertionError("closed TLS socket returned a certificate chain")


main()
