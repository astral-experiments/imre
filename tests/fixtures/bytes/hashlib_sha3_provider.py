import _sha3
import hashlib
import sys
from _sha3 import sha3_256, shake_256
from importlib import import_module

try:
    import _hashlib
except ImportError:
    _hashlib = None


def direct_import():
    import _sha3
    return _sha3


def current():
    assert import_module("_sha3") is _sha3 is sys.modules["_sha3"]
    assert sha3_256 is _sha3.sha3_256 and shake_256 is _sha3.shake_256
    assert _sha3._GIL_MINSIZE == 2048
    assert isinstance(_sha3.implementation, str) and _sha3.implementation

    fixed = ("sha3_224", "sha3_256", "sha3_384", "sha3_512")
    shakes = ("shake_128", "shake_256")
    lookup = hashlib.__get_builtin_constructor
    cache = hashlib.__builtin_constructor_cache
    for group in (fixed, shakes):
        cache.clear()
        lookup(group[0])
        assert set(cache) == set(group)
        for name in group:
            cls = getattr(_sha3, name)
            builtin = not hasattr(_hashlib, "openssl_" + name)
            assert lookup(name) is cls and (getattr(hashlib, name) is cls) == builtin
            assert cls.__module__ == "_sha3" and cls.__name__ == cls.__qualname__ == name
            assert cls.__new__.__self__ is cls
            h = cls(data=bytearray(b"ab"), usedforsecurity=False)
            public = hashlib.new(name, b"abc")
            assert (type(public) is cls) == builtin
            assert type(h) is cls and repr(h) == f"<_sha3.{name} object at 0x{id(h):x}>"
            assert h.name == name and cls.name.__get__(h) == name
            assert cls.copy.__objclass__ is cls and cls._suffix.__objclass__ is cls
            branch = cls.copy(h)
            assert type(branch) is cls and cls.update(h, b"c") is None
            assert h.block_size == public.block_size and h.digest_size == public.digest_size
            assert h._rate_bits == 8 * h.block_size and h._capacity_bits == 1600 - h._rate_bits
            assert h._capacity_bits == 2 * int(name.split("_")[1])
            if name in shakes:
                assert h.digest_size == 0 and h._suffix == b"\x1f"
                assert cls.digest(h, length=32) == public.digest(32) != branch.digest(32)
                assert h.hexdigest(32) == public.hexdigest(32) and h.digest(0) == b""
            else:
                assert h.digest_size == int(name[5:]) // 8 and h._suffix == b"\x06"
                assert cls.digest(h) == public.digest() != branch.digest()
                assert h.hexdigest() == public.hexdigest()

    try:
        _sha3.sha3_224.update(_sha3.sha3_256(), b"a")
    except TypeError:
        pass
    else:
        raise AssertionError("SHA3 descriptor accepted another SHA3 type")

    try:
        _sha3.shake_128.name = "renamed"
    except TypeError:
        pass
    else:
        raise AssertionError("SHAKE class accepted mutation")

    lookup("sha3_256")
    sys.modules["_sha3"] = None
    try:
        assert lookup("sha3_256") is _sha3.sha3_256
        for load in (direct_import, lambda: import_module("_sha3")):
            try:
                load()
            except ModuleNotFoundError as error:
                assert error.name == "_sha3"
            else:
                raise AssertionError("import ignored a masked SHA3 provider")
        cache.clear()
        try:
            lookup("sha3_256")
        except ValueError as error:
            assert error.args == ("unsupported hash type sha3_256",)
        else:
            raise AssertionError("empty cache ignored a masked SHA3 provider")
        assert hashlib.sha3_256(b"a").digest() == _sha3.sha3_256(b"a").digest()
    finally:
        sys.modules["_sha3"] = _sha3
    assert lookup("sha3_256") is _sha3.sha3_256


if __name__ == "__main__":
    current()
