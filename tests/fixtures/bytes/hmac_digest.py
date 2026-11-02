import hashlib
import hmac
import sys
from types import SimpleNamespace


def current():
    # RFC 4231, test case 2.
    key = b"Jefe"
    msg = b"what do ya want for nothing?"
    expected = bytes.fromhex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    assert hmac.digest(key=memoryview(key), msg=bytearray(msg), digest="SHA-256") == expected
    assert hmac.digest(memoryview(key), bytearray(msg), hashlib.sha256) == expected

    from _md5 import md5
    # RFC 2202, test case 2, with the explicit portable constructor.
    assert hmac.digest(key, msg, md5) == bytes.fromhex("750c783e6ab0b503eaa86e310a5db738")

    class HiddenBytes(bytes):
        def __buffer__(self, flags):
            raise LookupError("buffer override")

    for key_arg, msg_arg in ((HiddenBytes(key), msg), (key, HiddenBytes(msg))):
        try:
            hmac.digest(key_arg, msg_arg, "sha256")
        except (LookupError, NotImplementedError):
            pass
        else:
            raise AssertionError("overridden buffer was read as base bytes")

    calls = []

    def factory(data=b""):
        calls.append(data)
        return hashlib.sha256(data)

    assert hmac.digest(key, msg, factory) == expected
    assert hmac.digest(key, msg, SimpleNamespace(new=factory)) == expected
    assert calls == [b"", b"", b"", b""]
    long_key = b"a" * 100
    assert hmac.digest(long_key, msg, factory) == hmac.new(long_key, msg, "sha256").digest()
    assert calls[-3:] == [b"", b"", long_key]

    def broken(data=b""):
        raise LookupError("digest provider")

    try:
        hmac.digest(key, msg, broken)
    except LookupError as error:
        assert error.args == ("digest provider",)
    else:
        raise AssertionError("digest provider exception was lost")

    try:
        hmac.digest(key, msg, "shake_128")
    except ValueError:
        pass
    else:
        raise AssertionError("variable-length digest was accepted")


def provider_names():
    import _hashlib

    for name in ("ripemd160", "md5-sha1", "sm3"):
        # PBKDF2 with one iteration is HMAC(key, salt + block_number).
        try:
            expected = _hashlib.pbkdf2_hmac(name, b"k", b"m", 1)
        except ValueError:
            continue
        assert hmac.digest(memoryview(b"k"), b"m\0\0\0\1", name) == expected


def legacy_policy():
    import _hashlib
    from _md5 import md5

    for name in ("ripemd160", "md4"):
        try:
            hmac.digest(b"k", b"m", name)
        except ValueError as exc:
            assert type(exc) is ValueError
            assert exc.args == ("[digital envelope routines] unsupported",)
        else:
            raise AssertionError("configured MAC policy was ignored")

    try:
        hmac.digest(b"k", b"m", _hashlib.openssl_sha256)
    except _hashlib.UnsupportedDigestmodError:
        pass
    else:
        raise AssertionError("an unavailable explicit OpenSSL constructor was replaced")
    expected = bytes.fromhex("b60090e3052297aeb5a080889ce2fc4bca957e756faeb4df7d31800ca1e771ec")
    assert hmac.digest(b"Jefe", b"what do ya want for nothing?", md5) == bytes.fromhex("750c783e6ab0b503eaa86e310a5db738")
    try:
        hmac.digest(memoryview(b"k"), b"m", hashlib.sha256)
    except AttributeError as exc:
        assert "ljust" in str(exc)
    else:
        raise AssertionError("an unavailable public OpenSSL SHA256 skipped the callable fallback")
    assert hmac.digest(b"k", b"m", "sha256") == expected


if __name__ == "__main__":
    if sys.argv[1:] == ["legacy-policy"]:
        legacy_policy()
    else:
        current()
        try:
            import _hashlib
        except ImportError:
            pass
        else:
            provider_names()
