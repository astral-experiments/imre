from hashlib import new


def digest(name: str, data: bytes, secure: bool) -> bytes:
    original = new(name, usedforsecurity=secure)
    original.update(b"prefix")
    copy = original.copy()
    copy.update(data)
    assert original.digest() == new(name, b"prefix", usedforsecurity=secure).digest()
    return copy.digest()


for name in ("sha256", "sha512", "blake2b", "sha3_224", "md5", "ssl3-md5", "1.2.840.113549.2.5"):
    print(digest(name, b"abc", True))

for name in ("unknown", "shake_128"):
    try:
        digest(name, b"", False)
    except (TypeError, ValueError) as error:
        print(str(error))
