import hashlib


class Length:
    def __index__(self) -> int:
        print("index")
        return 3


assert hashlib.shake_128().hexdigest(16) == "7f9c2ba4e88f827d616045507605853e"
assert hashlib.shake_256().hexdigest(16) == "46b9dd2b0ba88d13233b3feb743eeb24"

for name in ["shake_128", "SHAKE128", "shake-128", "2.16.840.1.101.3.4.2.11",
             "shake_256", "SHAKE256", "shake-256", "2.16.840.1.101.3.4.2.12"]:
    h = hashlib.new(name, b"ab")
    saved = h.copy()
    h.update(b"c")
    assert h.name in hashlib.algorithms_guaranteed
    assert h.name in hashlib.algorithms_available
    assert h.digest_size == 0
    for n in [0, 1, 16, 32, 136, 168, 300]:
        print((h.name, h.block_size, h.hexdigest(length=n)))
        assert saved.digest(n) == hashlib.new(name, b"ab").digest(n)
        assert h.digest(n) == hashlib.new(name, b"abc").digest(n)
    assert h.digest(Length()) == h.digest(3)
    assert h.hexdigest(False) == ""
    try:
        h.digest()
    except TypeError as exc:
        print(str(exc))

for fixed in [hashlib.sha256(), hashlib.md5()]:
    try:
        fixed.digest(Length())
    except TypeError as exc:
        print(str(exc))
    try:
        fixed.hexdigest(length=3)
    except TypeError as exc:
        print(str(exc))

try:
    hashlib.shake_128().hexdigest(2**128)
except OverflowError:
    print("length overflow")
