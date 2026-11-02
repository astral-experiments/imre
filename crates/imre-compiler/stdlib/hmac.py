# One-shot fallback from CPython 3.14.5 Lib/hmac.py (PSF-2.0).
# See stdlib/docstrings/LICENSE.

try:
    import _hashlib as _hashopenssl
except ImportError:
    _hashopenssl = None
    _functype = None
else:
    _functype = type(_hashopenssl.openssl_sha256)

try:
    import _hmac
except ImportError:
    _hmac = None

_trans_36 = bytes(x ^ 0x36 for x in range(256))
_trans_5c = bytes(x ^ 0x5c for x in range(256))


def digest(key, msg, digest):
    """Return the HMAC of msg with key using the specified digest."""
    if _hashopenssl and isinstance(digest, (str, _functype)):
        try:
            result = _digest_native(key, msg, digest)
        except OverflowError:
            return _compute_digest_fallback(key, msg, digest)
        if result is not NotImplemented:
            return result

    if _hmac and isinstance(digest, str):
        try:
            return _hmac.compute_digest(key, msg, digest)
        except (OverflowError, _hmac.UnknownHashError):
            pass
    return _compute_digest_fallback(key, msg, digest)


def _compute_digest_fallback(key, msg, digest):
    if callable(digest):
        digest_cons = digest
    elif isinstance(digest, str):
        def digest_cons(data=b''):
            import hashlib
            return hashlib.new(digest, data)
    else:
        def digest_cons(data=b''):
            return digest.new(data)

    inner = digest_cons()
    outer = digest_cons()
    blocksize = getattr(inner, 'block_size', 64)
    if len(key) > blocksize:
        key = digest_cons(key).digest()
    key = key.ljust(blocksize, b'\0')
    inner.update(key.translate(_trans_36))
    outer.update(key.translate(_trans_5c))
    inner.update(msg)
    outer.update(inner.digest())
    return outer.digest()
