import ssl
import sys

certdata = sys.argv[1]
client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
client_ctx.load_verify_locations(certdata + "/ssl_cert.pem")
server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
server_ctx.load_cert_chain(certdata + "/keycert.pem")


def connect(hostname):
    cin, cout = ssl.MemoryBIO(), ssl.MemoryBIO()
    sin, sout = ssl.MemoryBIO(), ssl.MemoryBIO()
    client = client_ctx.wrap_bio(cin, cout, server_hostname=hostname)
    server = server_ctx.wrap_bio(sin, sout, server_side=True)
    for _ in range(10):
        complete = 0
        for peer, outgoing, incoming in ((client, cout, sin), (server, sout, cin)):
            try:
                peer.do_handshake()
                complete += 1
            except ssl.SSLWantReadError:
                pass
            incoming.write(outgoing.read())
        if complete == 2:
            break
    else:
        raise AssertionError("TLS handshake did not finish")
    return client, server, cin, cout, sin, sout


def want_read(operation):
    try:
        operation()
    except ssl.SSLWantReadError as error:
        assert error.errno == ssl.SSL_ERROR_WANT_READ
    else:
        raise AssertionError("expected SSLWantReadError")


client, server, cin, cout, sin, sout = connect("localhost")
with open(certdata + "/ssl_cert.pem") as cert:
    assert client.getpeercert(binary_form=True) == ssl.PEM_cert_to_DER_cert(cert.read())
identity = client.getpeercert()["subjectAltName"]
assert ("DNS", "localhost") in identity
assert server.getpeercert(True) is None
print("peer", identity)

request = b"Q\x00\xff?!"
assert client.write(data=b"") == 0
assert client.write(data=memoryview(request)) == len(request)
record = cout.read()
sin.write(record[:1])
assert server.pending() == 0
want_read(server.read)
sin.write(record[1:])
assert sin.pending > 0 and server.pending() == 0
assert server.read(0) == b""
first = server.read(len=2)
assert first == request[:2] and server.pending() == len(request) - 2
buf = bytearray(b"*****")
n = server.read(len=-1, buffer=memoryview(buf)[1:-1])
assert n == 3 and buf == b"*\xff?!*" and server.pending() == 0
print("request", first, n, bytes(buf))

reply = bytearray(b"R\x00\xfe")
assert server.write(reply) == len(reply)
cin.write(sout.read())
buf = bytearray(b"......")
n = client.read(len=0, buffer=buf)
assert n == len(reply) and buf == b"R\x00\xfe..." and client.pending() == 0
print("reply", n, bytes(buf))

want_read(client.unwrap)
sin.write(cout.read())
assert server.read() == b"" and server.read(5, buf) == 0
assert server.unwrap() is None
cin.write(sout.read())
assert client.unwrap() is None
try:
    client.write(b"closed")
except ssl.SSLError as error:
    assert error.reason == "PROTOCOL_IS_SHUTDOWN"
else:
    raise AssertionError("wrote after TLS shutdown")

client, server, cin, cout, sin, sout = connect("localhost")
cin.write_eof()
try:
    client.read()
except ssl.SSLEOFError as error:
    assert error.errno == ssl.SSL_ERROR_EOF
else:
    raise AssertionError("accepted TLS stream without close_notify")

try:
    connect("mismatch.invalid")
except ssl.SSLCertVerificationError as error:
    assert error.verify_code == 62
else:
    raise AssertionError("accepted wrong hostname")

# A record authenticated under one session cannot be read by a different peer.
client, server, cin, cout, sin, sout = connect("localhost")
other, peer, other_in, other_out, peer_in, peer_out = connect("localhost")
assert server.write(b"one session") == 11
record = sout.read()
other_in.write(record)
try:
    other.read()
except ssl.SSLError as error:
    assert error.library == "SSL" and error.errno == ssl.SSL_ERROR_SSL
    assert error.reason in ("RECORD_LAYER_FAILURE", "DECRYPTION_FAILED_OR_BAD_RECORD_MAC")
else:
    raise AssertionError("foreign record was accepted")
assert other.pending() == 0
alert = other_out.read()
assert alert
peer_in.write(alert)
try:
    peer.read()
except ssl.SSLError as error:
    assert error.library == "SSL" and error.errno == ssl.SSL_ERROR_SSL
    assert error.reason == "SSLV3_ALERT_BAD_RECORD_MAC"
else:
    raise AssertionError("missing bad-record alert")
cin.write(record)
assert client.read() == b"one session"
