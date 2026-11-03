import ssl
import sys

certdata = sys.argv[1]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
cert = certdata + "/ssl_cert.pem"
key = certdata + "/ssl_key.passwd.pem"
calls = []


def password():
    calls.append(ctx.protocol)
    return bytearray(b"somepass")


ctx.load_cert_chain(cert, key, password)
assert calls == [ssl.PROTOCOL_TLS_SERVER]
ctx.load_cert_chain(certdata + "/keycert.pem", password=password)
assert calls == [ssl.PROTOCOL_TLS_SERVER]

failure = RuntimeError("password callback failed")


def broken_password():
    raise failure


try:
    ctx.load_cert_chain(cert, key, broken_password)
except RuntimeError as exc:
    assert exc is failure
else:
    raise AssertionError("lost password callback error")

try:
    ctx.load_cert_chain(certdata + "/keycert2.pem", certdata + "/ssl_key.pem")
except ssl.SSLError as exc:
    assert (exc.library, exc.reason) == ("X509", "KEY_VALUES_MISMATCH")
else:
    raise AssertionError("accepted a mismatched key")

ctx.load_cert_chain(cert, key, b"somepass")
