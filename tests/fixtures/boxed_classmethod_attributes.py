def mark(obj):
    obj.__final__ = True
    return obj


class Base:
    @mark
    @classmethod
    def outer(cls):
        return cls

    @classmethod
    @mark
    def inner(cls):
        return cls


class Child(Base):
    pass


def current():
    class Auto:
        def __class_getitem__(cls, key):
            return cls, key

        def __init_subclass__(cls):
            pass

        @classmethod
        def explicit(cls):
            return cls

    class Derived(Auto):
        pass

    assert vars(vars(Auto)["__class_getitem__"]) == {}
    assert vars(vars(Auto)["__init_subclass__"]) == {}
    assert vars(vars(Auto)["explicit"])["__name__"] == "explicit"
    assert Derived[str] == (Derived, str)
    outer = Base.__dict__["outer"]
    inner = Base.__dict__["inner"]
    assert outer.__final__ is True and not hasattr(outer.__func__, "__final__")
    assert not hasattr(inner, "__final__") and inner.__func__.__final__ is True
    assert Child.outer() is Child and Child().inner() is Child
    from inspect import getattr_static
    token = object()
    outer.tag = token
    assert outer.__dict__["tag"] is token and not hasattr(outer.__func__, "tag")
    assert getattr_static(outer, "tag") is token
    assert getattr_static(Child, "outer") is outer
    replacement = {"tag": 7}
    outer.__dict__ = replacement
    assert outer.__dict__ is replacement and outer.tag == 7
    del outer.tag
    assert "tag" not in replacement and not hasattr(outer, "tag")
    try:
        outer.__func__ = inner
    except AttributeError:
        pass
    else:
        raise AssertionError("writable classmethod.__func__")
