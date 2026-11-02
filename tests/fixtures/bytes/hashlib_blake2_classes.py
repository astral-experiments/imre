import hashlib

for cls, other, limit, salt in (
    (hashlib.blake2s, hashlib.blake2b, 32, 8),
    (hashlib.blake2b, hashlib.blake2s, 64, 16),
):
    h = cls(b"abc")
    name = cls.__name__
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
