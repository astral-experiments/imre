import ssl
import warnings

with warnings.catch_warnings(record=True) as recorded:
    warnings.simplefilter("always")
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS)
assert [str(item.message) for item in recorded] == ["ssl.PROTOCOL_TLS is deprecated"]
assert ctx.protocol is ssl.PROTOCOL_TLS
assert ctx.verify_mode is ssl.CERT_NONE and ctx.check_hostname is False


class OptionalMode:
    def __index__(self):
        ctx.check_hostname = True
        return ssl.CERT_OPTIONAL.value


ctx.verify_mode = OptionalMode()
assert ctx.check_hostname is True and ctx.verify_mode is ssl.CERT_OPTIONAL
try:
    ctx.verify_mode = ssl.CERT_NONE
except ValueError:
    pass
else:
    raise AssertionError("accepted CERT_NONE with hostname checks enabled")

ctx.check_hostname = False
assert ctx.verify_mode is ssl.CERT_OPTIONAL
ctx.verify_mode = ssl.CERT_NONE
ctx.check_hostname = True
assert ctx.verify_mode is ssl.CERT_REQUIRED


with warnings.catch_warnings(record=True) as recorded:
    warnings.simplefilter("always")
    try:
        fixed = ssl.SSLContext(ssl.PROTOCOL_TLSv1_2)
    except ValueError as exc:
        # OpenSSL can omit its deprecated method selectors.
        assert str(exc) == "invalid or unsupported protocol version 5"
        assert not recorded
    else:
        assert [str(item.message) for item in recorded] == ["ssl.PROTOCOL_TLSv1_2 is deprecated"]
        assert fixed.protocol is ssl.PROTOCOL_TLSv1_2
        assert fixed.verify_mode is ssl.CERT_NONE and fixed.check_hostname is False
        for bound in ("minimum_version", "maximum_version"):
            try:
                setattr(fixed, bound, ssl.TLSVersion.TLSv1_2)
            except ValueError as exc:
                assert "doesn't support modification" in str(exc)
            else:
                raise AssertionError("changed a fixed-version context's protocol bound")
