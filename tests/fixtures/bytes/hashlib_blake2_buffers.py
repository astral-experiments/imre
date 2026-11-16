from hashlib import blake2b, blake2s, new


def keyed_digest(data: bytes, *, key: bytearray, digest_size: int) -> str:
    return new("blake2s", data, key=key, digest_size=digest_size).hexdigest()


data = memoryview(b"abc")
key = bytearray(b"key")
salt = memoryview(b"salt")
b = blake2b(data, key=key, salt=salt, digest_size=16)
s = blake2s(data, key=key, salt=salt, digest_size=16)
assert b.hexdigest() == "e005831284aa785843e7057af89880fd"
assert s.hexdigest() == "99aedd2e1130191fbf4d1b3910646c51"
assert new("blake2b", data, key=key, salt=salt, digest_size=16).digest() == b.digest()
assert new("blake2s", data, key=key, salt=salt, digest_size=16).digest() == s.digest()

options = {"key": bytearray(b"k"), "digest_size": 16}
assert keyed_digest(b"abc", **options) == "f335237426ff5582d0c607ef240684b5"
