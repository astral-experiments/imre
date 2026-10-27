import hashlib
import hmac


def check():
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
