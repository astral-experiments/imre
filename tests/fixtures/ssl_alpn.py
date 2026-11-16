import ssl
import sys


certdata = sys.argv[1]
client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
client_ctx.load_verify_locations(certdata + "/ssl_cert.pem")
server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_ctx.load_cert_chain(certdata + "/keycert.pem")


def connect(offers, preferences, expected):
    client_ctx.set_alpn_protocols(offers)
    cin, cout = ssl.MemoryBIO(), ssl.MemoryBIO()
    sin, sout = ssl.MemoryBIO(), ssl.MemoryBIO()
    client = client_ctx.wrap_bio(cin, cout, server_hostname="localhost")
    server = server_ctx.wrap_bio(sin, sout, server_side=True)
    assert client.selected_alpn_protocol() is None
    assert server.selected_alpn_protocol() is None

    # The client offer was copied into its SSL. The server consults its current
    # context preferences when OpenSSL processes this connection's ClientHello.
    client_ctx.set_alpn_protocols(["later-offer"])
    server_ctx.set_alpn_protocols(preferences)
    for _ in range(10):
        done = 0
        for peer, outgoing, incoming in ((client, cout, sin), (server, sout, cin)):
            try:
                peer.do_handshake()
                done += 1
            except ssl.SSLWantReadError:
                pass
            incoming.write(outgoing.read())
        if done == 2:
            break
    else:
        raise AssertionError("TLS handshake did not complete")
    server_ctx.set_alpn_protocols(["later-preference"])
    assert client.selected_alpn_protocol() == server.selected_alpn_protocol() == expected
    with open(certdata + "/ssl_cert.pem") as cert:
        assert client.getpeercert(True) == ssl.PEM_cert_to_DER_cert(cert.read())
    assert client.write(b"ping") == 4
    sin.write(cout.read())
    assert server.read(4) == b"ping"


connect(["http/1.1", "h2"], ["h2", "http/1.1"], "h2")
connect(["http/1.1"], ["h2", "http/1.1"], "http/1.1")
connect(["one"], ["two"], None)
connect([], ["http/1.1"], None)
connect(["http/1.1"], [], None)
print("ALPN selection and verified application data ok")
