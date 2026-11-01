import hashlib
import hmac


def current():
    for name in ("SHA-1", "sha2-256"):
        h = hashlib.new(name=name, data=bytearray(b"abc"), usedforsecurity=False)
        print(h.name, h.hexdigest(), h.digest_size)
        print(hashlib.new(name, string=b"abc").digest() == h.digest())
        print(h.name in hashlib.algorithms_available, h.name in hashlib.algorithms_guaranteed)
    available = hashlib.algorithms_available
    available.add("missing-hash")
    print(available is hashlib.algorithms_available, "missing-hash" in hashlib.algorithms_available,
          "missing-hash" in hashlib.algorithms_guaranteed)
    try:
        hashlib.new("missing-hash")
    except ValueError as exc:
        print(str(exc))
    available.discard("missing-hash")
    for call in (
        lambda: hashlib.new(12),
        lambda: hashlib.new("sha1", b"a", data=b"b"),
        lambda: hashlib.new("sha1", data=b"a", string=b"b"),
    ):
        try:
            call()
        except TypeError as exc:
            print(str(exc))
    try:
        hmac.new(b"k", digestmod="new")
    except ValueError:
        print("unavailable HMAC digest")
