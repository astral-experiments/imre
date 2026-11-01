import hashlib


class Flag:
    def __bool__(self) -> bool:
        print("flag checked")
        return False


def payload() -> bytes:
    print("payload evaluated")
    return b"abc"


expected = hashlib.sha256(b"abc").hexdigest()
assert hashlib.sha256(string=b"abc").hexdigest() == expected
assert hashlib.sha256(data=b"abc").hexdigest() == expected
assert hashlib.new(string=b"abc", name="SHA256").hexdigest() == expected
assert hashlib.sha512(usedforsecurity=Flag(), string=payload()).digest() == hashlib.sha512(b"abc").digest()

try:
    hashlib.sha224(usedforsecurity=Flag(), data=b"", string=payload())
except TypeError as exc:
    print(str(exc))

try:
    hashlib.new("sha384", b"", string=b"")
except TypeError as exc:
    print(str(exc))

try:
    hashlib.sha1("unencoded", usedforsecurity=Flag())
except TypeError as exc:
    print(str(exc))

try:
    hashlib.new("missing", None)
except TypeError as exc:
    print(str(exc))

h = hashlib.sha256(b"abc")
try:
    h.update("unencoded")
except TypeError as exc:
    print(str(exc))
try:
    h.update(123)
except TypeError as exc:
    print(str(exc))
assert h.hexdigest() == expected
