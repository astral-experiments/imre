import ssl
import sys
import warnings

certfile = sys.argv[1] + "/keycert.pem"
client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
client_ctx.load_verify_locations(certfile)
server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_ctx.load_cert_chain(certfile)


def handshake(client_ctx, server_ctx, hostname):
    # Each SSL object retains the BIOs, including when another SSL uses the same BIO.
    to_client, to_server = ssl.MemoryBIO(), ssl.MemoryBIO()
    client = client_ctx.wrap_bio(to_client, to_server, server_hostname=hostname)
    server = server_ctx.wrap_bio(to_server, to_client, server_side=True)
    del to_client, to_server
    for _ in range(5):
        for peer in (client, server):
            try:
                peer.do_handshake()
            except ssl.SSLWantReadError:
                pass
    client.do_handshake()
    server.do_handshake()
    return client, server


client, server = handshake(client_ctx, server_ctx, "localhost")

try:
    client.unwrap()
except ssl.SSLWantReadError as exc:
    assert exc.errno == 2
else:
    raise AssertionError("client finished shutdown before the server sent close_notify")
assert server.unwrap() is None
assert client.unwrap() is None

incoming = ssl.MemoryBIO()
truncated = client_ctx.wrap_bio(incoming, ssl.MemoryBIO(), server_hostname="localhost")
incoming.write_eof()
try:
    truncated.do_handshake()
except ssl.SSLEOFError as exc:
    assert exc.errno == 8
else:
    raise AssertionError("accepted EOF during the TLS handshake")


# The generic protocol chooses the role per connection. A client can verify the
# certificate chain even when it does not check the hostname.
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    generic = ssl.SSLContext(ssl.PROTOCOL_TLS)
generic.load_cert_chain(certfile)
handshake(client_ctx, generic, "localhost")
generic.verify_mode = ssl.CERT_REQUIRED
try:
    handshake(generic, server_ctx, "other.example")
except ssl.SSLCertVerificationError:
    pass
else:
    raise AssertionError("accepted an untrusted certificate with hostname checking disabled")
generic.load_verify_locations(certfile)
handshake(generic, server_ctx, "other.example")
