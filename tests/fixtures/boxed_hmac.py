import hashlib
import hmac


def check():
    import sys
    from _operator import _compare_digest
    try:
        from _hashlib import compare_digest as backend
    except ImportError:
        backend = _compare_digest
    assert hmac.compare_digest is backend
    for compare in (_compare_digest, hmac.compare_digest):
        owner = compare.__self__
        sys.modules.pop(owner.__name__)
        try:
            fresh = __import__(owner.__name__)
            new_compare = getattr(fresh, compare.__name__)
            assert fresh is not owner and new_compare is not compare
            assert new_compare.__self__ is fresh and compare.__self__ is owner
        finally:
            sys.modules[owner.__name__] = owner
        view = memoryview(b"xabc")
        assert compare(view[1:], b"abc")
        assert not compare(view[1:], bytearray(b"xbc"))
        try:
            compare(view[::2], b"xb")
        except BufferError:
            pass
        else:
            raise AssertionError("noncontiguous memoryview accepted")

    data = bytearray(b"abc")
    digest = hashlib.sha1(data)
    digest.update(data)
    mac = hmac.new(data, data, "sha1")
    mac.update(data)
    data[0] = 90
    print(digest.hexdigest(), mac.hexdigest())
    print(hmac.compare_digest(data, b"Zbc"), hmac.compare_digest(b"abc", data),
          hmac.compare_digest(data, bytearray(b"Zbc")))

    key = bytearray(b"x")
    lengths = []

    def grow_key(data=b""):
        lengths.append(len(data))
        if len(key) == 1:
            key.extend(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789..")
        return hashlib.sha1(data)

    grown = hmac.new(key, b"message", grow_key)
    print(lengths, grown.hexdigest() == hmac.new(bytes(key), b"message", "sha1").hexdigest())

    calls = []

    def lazy_sha1(data=b""):
        calls.append(len(data))
        return hashlib.sha1(data)

    key = b"local-probe-key"
    derived = hmac.new(key, digestmod=lazy_sha1)
    derived.update(b"cookie-session")
    mac = hmac.new(derived.digest(), msg=b"eyJ2aXNpdHMiOjF9.ZVPxAA", digestmod=lazy_sha1)
    signature = mac.digest()
    print(derived.hexdigest(), mac.hexdigest(), calls)
    print(mac.name, mac.digest_size, mac.block_size, isinstance(mac, hmac.HMAC))
    print(hmac.compare_digest(signature, mac.digest()), hmac.compare_digest(signature, b""))
    changed = mac.copy()
    changed.update(b"tampered")
    print(hmac.compare_digest(signature, changed.digest()), signature == mac.digest())

    # RFC 2202 case 6 hashes the overlong key before applying the pads.
    long_key = bytes([0xaa for _ in range(80)])
    long_mac = hmac.HMAC(
        long_key, b"Test Using Larger Than Block-Size Key - Hash Key First", lazy_sha1
    )
    print(long_mac.hexdigest(), calls)
    sha256_mac = hmac.new(b"key", b"message", "sha256")
    print(
        sha256_mac.name,
        sha256_mac.digest_size,
        sha256_mac.block_size,
        sha256_mac.hexdigest(),
    )
    for name in ["sha224", "sha384", "sha512", "sha3_224", "sha3_256", "sha3_384", "sha3_512"]:
        factory = getattr(hashlib, name)
        for size in [64, 65, 128, 129, factory().block_size + 1]:
            key = b"X" * size
            factory_mac = hmac.new(key, b"message", factory)
            print(factory_mac.name, factory_mac.digest_size, factory_mac.block_size, factory_mac.hexdigest())
            assert factory_mac.digest() == hmac.new(key, b"message", name).digest()
    for _ in range(5000):
        _discard = [None]
    print(long_mac.hexdigest(), mac.hexdigest())
    print(hmac.compare_digest("abc", "abc"), hmac.compare_digest("abc", "abD"))
    print(hmac.compare_digest(b"abc", b"dbc"), hmac.compare_digest(b"abc", b"abd"))
    for a, b in (("é", "é"), ("a", b"a")):
        try:
            hmac.compare_digest(a, b)
        except TypeError as exc:
            print(str(exc))

    class Factory:
        def __bool__(self):
            raise ValueError("factory truth")

        def __call__(self):
            raise AssertionError("digest constructor ran")

    try:
        hmac.new(key, digestmod=Factory())
    except ValueError as exc:
        print(str(exc))

    # RFC 4231 section 4.3, Test Case 2 (HMAC-SHA-256).
    key = b"Jefe"
    message = b"what do ya want for nothing?"
    expected_sha256 = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    for constructor in (hmac.new, hmac.HMAC):
        mac = constructor(key, message[:12], "SHA-256")
        copy = mac.copy()
        mac.update(message[12:])
        copy.update(message[12:])
        assert mac.hexdigest() == expected_sha256
        assert copy.hexdigest() == expected_sha256
        assert constructor(b"k", b"abc", "sHa3_256").digest() == hmac.new(b"k", b"abc", "SHA3-256").digest()
        for invalid in ("BLAKE2B", "Blake2s", "UnknownDigestName"):
            try:
                constructor(b"k", b"abc", invalid)
            except ValueError as exc:
                if invalid == "UnknownDigestName":
                    assert str(exc) == "unsupported hash type UnknownDigestName"
            else:
                raise AssertionError("unsupported HMAC digest name accepted")
        for name in ("blake2b", "blake2s"):
            if name in hashlib.algorithms_available:
                factory = getattr(hashlib, name)
                # CPython test_hmac.BuiltinMiscellaneousTests uses an independent
                # HMAC computation for BLAKE2. Apply the pads from RFC 2104 §2.
                padded = key + b"\x00" * (factory().block_size - len(key))
                inner = factory(bytes(x ^ 0x36 for x in padded) + message).digest()
                expected = factory(bytes(x ^ 0x5c for x in padded) + inner).digest()
                assert constructor(key, message, name).digest() == expected
                assert constructor(key, message, factory).digest() == expected
