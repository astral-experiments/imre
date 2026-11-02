import hashlib


def current():
    try:
        import _hashlib
    except ImportError:
        assert not hasattr(hashlib, "pbkdf2_hmac") and not hasattr(hashlib, "scrypt")
        return

    from hashlib import pbkdf2_hmac

    assert pbkdf2_hmac is _hashlib.pbkdf2_hmac
    assert "pbkdf2_hmac" in hashlib.__all__ and "scrypt" not in hashlib.__all__
    assert "sha256" in _hashlib.openssl_md_meth_names
    key = pbkdf2_hmac("sha256", b"pass\0word", b"salt", 2, dklen=16)
    assert type(key) is bytes and key == bytes.fromhex("92967dbecf561331d08f7e43e4bcd97f")
    try:
        pbkdf2_hmac("missing-digest", b"pass", b"salt", 1)
    except ValueError as error:
        assert type(error) is _hashlib.UnsupportedDigestmodError
        assert "unsupported" in str(error)
    else:
        raise AssertionError("PBKDF2 accepted an unknown digest")
    for name, expected in [
        ("sha256\0suffix", ValueError),
        ("\ud800", UnicodeEncodeError),
        (None, TypeError),
    ]:
        try:
            pbkdf2_hmac(name, b"pass", b"salt", 1)
        except Exception as error:
            assert type(error) is expected
        else:
            raise AssertionError("invalid digest name accepted")

    if hasattr(_hashlib, "scrypt"):
        from hashlib import scrypt

        assert scrypt is _hashlib.scrypt
        if not _hashlib.get_fips_mode():
            key = hashlib.scrypt(b"", salt=b"", n=16, r=1, p=1, dklen=16)
            assert type(key) is bytes and key == bytes.fromhex("77d6576238657b203b19ca42c18a0497")
            try:
                scrypt(b"", salt=b"", n=3, r=1, p=1)
            except ValueError:
                pass
            else:
                raise AssertionError("scrypt accepted a non-power-of-two N")
    else:
        assert not hasattr(hashlib, "scrypt")

    digest_controls(_hashlib)


def digest_controls(provider):
    factory = provider.openssl_sha256
    assert provider._constructors[factory] == "sha256" and factory.__self__ is provider
    h = factory(b"a", usedforsecurity=False)
    assert type(h) is provider.HASH and (h.name, h.block_size, h.digest_size) == ("sha256", 64, 32)
    h.update(bytearray(b"b"))
    branch = h.copy()
    assert h.digest() == branch.digest()
    h.update(b"c")
    branch.update(b"x")
    assert h.hexdigest() == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    assert branch.hexdigest() == "b2bdab4230154046ad0b7dfa830a9260caa5bd6ae230e582a2244ab4bf0b4ca5"
    assert provider.new("SHA-256", data=b"abc").digest() == h.digest()


if __name__ == "__main__":
    current()
