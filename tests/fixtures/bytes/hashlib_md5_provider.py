import hashlib
import importlib
import sys
import _md5


def check():
    old = _md5
    factory, cls = old.md5, old.MD5Type
    assert type(factory) is type(len) and factory.__self__ is old
    assert factory.__name__ == factory.__qualname__ == cls.__name__ == "md5"
    assert factory.__module__ == cls.__module__ == "_md5"
    assert cls.__new__ is object.__new__
    assert old._GIL_MINSIZE == 2048
    assert hashlib.__get_builtin_constructor("MD5") is factory
    try:
        import _hashlib
    except ImportError:
        assert hashlib.md5 is factory
        assert type(hashlib.new("md5")) is cls
    else:
        assert hashlib.md5 is _hashlib.openssl_md5
        assert type(hashlib.new("md5")) is _hashlib.HASH

    h = factory(data=bytearray(b"a"), usedforsecurity=False)
    assert type(h) is cls and h.name == "md5"
    assert (h.block_size, h.digest_size) == (64, 16)
    for allocate, args in ((cls, ()), (object.__new__, (cls,))):
        try:
            allocate(*args)
        except TypeError as exc:
            assert str(exc) == "cannot create '_md5.md5' instances"
        else:
            raise AssertionError("allocated an uninitialized MD5")

    try:
        sys.modules["_md5"] = None
        assert hashlib.__get_builtin_constructor("md5") is factory
        assert type(factory(b"a")) is type(h.copy()) is cls
        del sys.modules["_md5"]
        fresh = importlib.import_module("_md5")
        assert fresh is not old and fresh.MD5Type is not cls
        assert fresh.md5.__self__ is fresh and type(fresh.md5()) is fresh.MD5Type
        assert fresh.MD5Type.copy.__objclass__ is fresh.MD5Type
        assert cls.name.__objclass__ is cls
        for method in (fresh.MD5Type.copy, fresh.MD5Type.name.__get__):
            try:
                method(h)
            except TypeError:
                pass
            else:
                raise AssertionError("accepted a foreign MD5 receiver")
        cls.update(h, b"bc")
        assert h.hexdigest() == "900150983cd24fb0d6963f7d28e17f72"
        assert h.copy().digest() == fresh.md5(string=b"abc").digest()
    finally:
        sys.modules["_md5"] = old


check()
print("portable MD5 provider")
