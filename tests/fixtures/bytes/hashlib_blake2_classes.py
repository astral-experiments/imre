import _blake2
import hashlib
import sys
from importlib import import_module

assert import_module("_blake2") is _blake2 is sys.modules["_blake2"]
assert _blake2._GIL_MINSIZE == 2048

for cls, other, limit, salt in (
    (_blake2.blake2s, _blake2.blake2b, 32, 8),
    (_blake2.blake2b, _blake2.blake2s, 64, 16),
):
    h = cls(b"abc")
    name = cls.__name__
    assert cls.__module__ == "_blake2" and getattr(hashlib, name) is cls
    assert hashlib.__get_builtin_constructor(name) is cls
    for attr in ("SALT_SIZE", "PERSON_SIZE", "MAX_KEY_SIZE", "MAX_DIGEST_SIZE"):
        assert getattr(_blake2, name.upper() + "_" + attr) == getattr(cls, attr)
    assert type(cls) is type and type(h) is cls and type(h.copy()) is cls
    assert type(hashlib.new(name)) is cls
    assert isinstance(h, cls) and not isinstance(h, other)
    assert cls.__bases__ == (object,) and cls.__mro__ == (cls, object)
    assert issubclass(cls, object) and not issubclass(cls, other)
    assert cls.SALT_SIZE == h.SALT_SIZE == salt
    assert cls.PERSON_SIZE == salt and cls.MAX_KEY_SIZE == cls.MAX_DIGEST_SIZE == limit
    assert cls.__qualname__ == name and repr(cls) == f"<class '{cls.__module__}.{name}'>"
    assert repr(h) == f"<{cls.__module__}.{name} object at 0x{id(h):x}>"
    assert cls.__new__(cls, b"abc").digest() == h.digest()
    assert h.__init__(b"new", digest_size=1) is None
    assert h.hexdigest() == cls.hexdigest(h)
    assert cls.update(h, b"d") is None
    assert h.digest() == cls(b"abcd").digest()
    assert cls.copy.__objclass__ is cls and cls.copy.__name__ == "copy"
    assert cls.name.__objclass__ is cls and cls.name.__get__(h) == name
    assert cls.digest_size.__get__(h, cls) == limit
    assert cls.name.__get__(None, cls) is cls.name
    try:
        cls.copy(other())
    except TypeError:
        pass
    else:
        raise AssertionError("descriptor accepted the other BLAKE2 type")
    try:
        cls.MAX_KEY_SIZE = 0
    except TypeError:
        pass
    else:
        raise AssertionError("native BLAKE2 type accepted mutation")
    try:
        class Derived(cls):
            pass
    except TypeError:
        pass
    else:
        raise AssertionError("native BLAKE2 type accepted subclass")


def direct_import():
    import _blake2
    return _blake2


hashlib.__builtin_constructor_cache.clear()
sys.modules["_blake2"] = None
try:
    for load in (direct_import, lambda: import_module("_blake2")):
        try:
            load()
        except ModuleNotFoundError as error:
            assert error.name == "_blake2"
        else:
            raise AssertionError("import ignored a masked BLAKE2 module")
    try:
        hashlib.new("blake2s")
    except ValueError as error:
        assert error.args == ("unsupported hash type blake2s",)
    else:
        raise AssertionError("empty constructor cache ignored a masked BLAKE2 module")
    assert type(hashlib.blake2s()) is _blake2.blake2s
finally:
    sys.modules["_blake2"] = _blake2
assert direct_import() is import_module("_blake2") is _blake2
assert type(hashlib.new("blake2s")) is _blake2.blake2s

new = hashlib.new
sys.modules["hashlib"] = None
try:
    assert new("blake2b", b"abc").digest() == _blake2.blake2b(b"abc").digest()
finally:
    sys.modules["hashlib"] = hashlib
