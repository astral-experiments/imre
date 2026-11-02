from array import array
import hashlib
import hmac


def current():
    for code, values in (("i", [-129, 256]), ("f", [0.5]), ("d", [-1.5]), ("w", "😀")):
        a = array(code, values)
        raw = a.tobytes()
        before = hashlib.sha256(a)
        assert before.digest() == hashlib.sha256(raw).digest()
        h = hashlib.new("sha256", a)
        a.append(values[0])
        h.update(a)
        assert h.hexdigest() == hashlib.sha256(raw + a.tobytes()).hexdigest()
        assert before.copy().digest() == hashlib.sha256(raw).digest()
    for name in ("MD5", "ssl3-md5", "SHA-1", "SHA-224", "sha2-256", "SHA-384", "2.16.840.1.101.3.4.2.3",
                 "SHA3-224", "sha3_256", "Sha3-384", "2.16.840.1.101.3.4.2.10"):
        h = hashlib.new(name=name, data=bytearray(b"abc"), usedforsecurity=False)
        print(h.name, h.hexdigest(), h.digest_size)
        cls = type(h)
        alias = h
        copied = h.copy()
        assert cls is type(copied) and alias.__class__ is cls
        assert isinstance(h, cls) and isinstance(cls, type)
        try:
            cls.value = False
        except TypeError as exc:
            assert "immutable type" in str(exc)
        else:
            raise AssertionError("hash class unexpectedly allowed mutation")
        assert h.name in repr(h) and cls.__module__ + "." + cls.__name__ in repr(h)
        assert f"0x{id(h):x}" in repr(alias) and repr(h) != repr(copied)
        if cls is getattr(hashlib, h.name):
            assert cls.__module__ == "_sha3" and name == h.name
            assert cls().name == name
        else:
            try:
                cls()
            except TypeError:
                pass
            else:
                raise AssertionError("hash class unexpectedly accepted direct construction")
        print(hashlib.new(name, string=b"abc").digest() == h.digest())
        print(h.name in hashlib.algorithms_available, h.name in hashlib.algorithms_guaranteed)
    # A provider may use its own native spelling for the constructor.
    for constructor in (hashlib.sha1, hashlib.sha256):
        name = constructor.__name__
        module = constructor.__self__
        print(name == constructor.__qualname__, constructor.__module__ == module.__name__)
        print(getattr(module, name) is constructor, type(constructor) is type(len))
        print(repr(constructor) == f"<built-in function {name}>")
    cache = getattr(hashlib, "__builtin_constructor_cache")
    calls = []

    def factory(data=b"", *, usedforsecurity=True):
        calls.append((data, usedforsecurity))
        return hashlib.sha256(data)

    cache["test-hash"] = factory
    try:
        result = hashlib.new("test-hash", b"abc", usedforsecurity=False)
        assert result.hexdigest() == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        assert calls == [(b"abc", False)]
    finally:
        del cache["test-hash"]

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

    h = hashlib.shake_128(bytearray(b"a"))

    class Length:
        def __index__(self):
            h.update(b"b")
            return 32

    print(h.digest(length=Length()) == hashlib.new("SHAKE-128", b"ab").digest(32))
    for digestmod in ("shake_128", hashlib.shake_256):
        try:
            hmac.new(b"key", b"abc", digestmod)
        except ValueError as exc:
            print(str(exc))
