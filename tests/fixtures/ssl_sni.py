import ssl
import sys
import warnings

certdata = sys.argv[1]
client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
client_ctx.load_verify_locations(certdata + "/ssl_cert.pem")
server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_ctx.load_cert_chain(certdata + "/keycert.pem")
names = []


def deny(peer, name, context):
    assert peer is server and context is server_ctx
    names.append(name)
    context.sni_callback = None
    return 112  # RFC 6066 unrecognized_name


for compatible in (True, False):
    if not compatible:
        client_ctx.maximum_version = ssl.TLSVersion.TLSv1_2
        server_ctx.minimum_version = ssl.TLSVersion.TLSv1_3
    server_ctx.sni_callback = deny
    cin, cout = ssl.MemoryBIO(), ssl.MemoryBIO()
    sin, sout = ssl.MemoryBIO(), ssl.MemoryBIO()
    client = client_ctx.wrap_bio(cin, cout, server_hostname="localhost")
    server = server_ctx.wrap_bio(sin, sout, server_side=True)
    try:
        client.do_handshake()
    except ssl.SSLWantReadError:
        pass
    sin.write(cout.read())
    try:
        server.read(100)  # Initial application I/O must enforce the same policy.
    except ssl.SSLError as error:
        assert error.reason == ("CALLBACK_FAILED" if compatible else "UNSUPPORTED_PROTOCOL")
    else:
        raise AssertionError("unexpected server acceptance")
    cin.write(sout.read())
    try:
        client.do_handshake()
    except ssl.SSLError as error:
        assert error.reason == ("TLSV1_UNRECOGNIZED_NAME" if compatible else "TLSV1_ALERT_PROTOCOL_VERSION")
    else:
        raise AssertionError("unexpected client acceptance")
    assert names == ["localhost"]
    assert (server_ctx.sni_callback is None) == compatible
print("client received the SNI callback's fatal alert")


# A general-purpose context can receive OpenSSL's servername callback when
# used as the client, too. Keep peer verification and the actual SSLObject.
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    generic = ssl.SSLContext(ssl.PROTOCOL_TLS)
generic.verify_mode = ssl.CERT_REQUIRED
generic.check_hostname = True
generic.load_verify_locations(certdata + "/ssl_cert.pem")
server_ctx.sni_callback = None
seen = []


def on_reply(peer, name, context):
    seen.append((peer is client, name, context is generic))


class SelectContext:
    def __call__(self, peer, name, context):
        self.peer = peer
        context.sni_callback = None

    def __del__(self):
        self.peer.context = selected


selected = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
selected.load_cert_chain(certdata + "/keycert.pem")
selected.set_alpn_protocols(["selected"])
server_ctx.set_alpn_protocols(["initial"])
generic.set_alpn_protocols(["selected", "initial"])
server_ctx.sni_callback = SelectContext()
generic.sni_callback = on_reply
cin, cout = ssl.MemoryBIO(), ssl.MemoryBIO()
sin, sout = ssl.MemoryBIO(), ssl.MemoryBIO()
client = generic.wrap_bio(cin, cout, server_hostname="localhost")
server = server_ctx.wrap_bio(sin, sout, server_side=True)
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
assert done == 2 and seen == [(True, "localhost", True)]
assert client.selected_alpn_protocol() == "selected"
assert client.write(b"ok") == 2
sin.write(cout.read())
assert server.read(2) == b"ok"



class RetiredPolicy:
    def __call__(self, peer, name, context):
        raise AssertionError("retired policy called")

    def __del__(self):
        seen.append(server_ctx.sni_callback is None)
        server_ctx.sni_callback = on_reply


server_ctx.sni_callback = RetiredPolicy()
server_ctx.sni_callback = deny
assert seen[-1] is True and server_ctx.sni_callback is deny
