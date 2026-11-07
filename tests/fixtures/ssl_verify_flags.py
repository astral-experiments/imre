import _ssl
import ssl
import struct

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
initial = ctx.verify_flags
assert isinstance(initial, ssl.VerifyFlags)
assert ssl.VerifyFlags(_ssl.VERIFY_CRL_CHECK_CHAIN) is ssl.VERIFY_CRL_CHECK_CHAIN
assert ssl.VERIFY_CRL_CHECK_CHAIN is ssl.VerifyFlags.__members__["VERIFY_CRL_CHECK_CHAIN"]
assert ssl.AlertDescription(_ssl.ALERT_DESCRIPTION_CLOSE_NOTIFY) is ssl.ALERT_DESCRIPTION_CLOSE_NOTIFY
assert ssl.SSLErrorNumber(_ssl.SSL_ERROR_WANT_READ) is ssl.SSL_ERROR_WANT_READ
assert type(_ssl.SSL_ERROR_WANT_READ) is int
assert not initial & ssl.VERIFY_CRL_CHECK_LEAF
assert bool(ssl.VERIFY_CRL_CHECK_LEAF)
other = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.verify_flags = ssl.VERIFY_CRL_CHECK_CHAIN
assert ctx.verify_flags == ssl.VERIFY_CRL_CHECK_CHAIN
assert other.verify_flags == initial


class Index:
    def __index__(self):
        ctx.verify_flags = ssl.VERIFY_ALLOW_PROXY_CERTS
        return (1 << 128) | ssl.VERIFY_X509_STRICT.value


ctx.verify_flags = Index()
assert ctx.verify_flags == ssl.VERIFY_X509_STRICT
ctx.verify_flags = -1
assert int(ctx.verify_flags) == (1 << (8 * struct.calcsize("L"))) - 1
ctx.verify_flags = ssl.VERIFY_DEFAULT
assert ctx.verify_flags is ssl.VERIFY_DEFAULT
try:
    ctx.verify_flags = None
except TypeError as exc:
    assert str(exc) == "argument must be int, not None"
else:
    raise AssertionError("accepted None for verify_flags")
assert ctx.verify_flags is ssl.VERIFY_DEFAULT
