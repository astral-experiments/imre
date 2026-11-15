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
    from inspect import getattr_static, signature
    from types import MethodType

    class Auto:
        slot = classmethod(object.__str__)

        def __class_getitem__(cls, key):
            return cls, key

        def __init_subclass__(cls):
            pass

        @classmethod
        def explicit(cls):
            """Return the class used to bind this method."""
            return cls

    class Derived(Auto):
        pass

    assert vars(vars(Auto)["__class_getitem__"]) == {}
    assert vars(vars(Auto)["__init_subclass__"]) == {}
    raw = vars(Auto)["explicit"]
    type_dict = type.__dict__["__dict__"].__get__
    for wrapper in (raw, staticmethod(raw.__func__)):
        kind = type(wrapper)
        namespace = type_dict(kind)
        member = namespace["__dict__"]
        assert namespace is not type_dict(kind) and member is vars(kind)["__dict__"]
        assert member.__objclass__ is kind and member.__get__(wrapper) is vars(wrapper)
        assert member.__doc__ is None
        for name in ("__func__", "__wrapped__"):
            field = namespace[name]
            assert getattr_static(wrapper, name) is field
            assert field.__get__(wrapper) is raw.__func__ and field.__doc__ is None
        assert ("__call__" in namespace) is (kind is staticmethod)
        assert namespace["__get__"] is kind.__get__
        assert namespace["__doc__"] == kind.__doc__ and kind.__doc__ is not kind.__doc__
        generic = namespace["__class_getitem__"]
        assert generic.__objclass__ is kind and generic.__get__(None, kind)(str) == kind[str]
    bound = Derived.explicit
    assert type(bound) is MethodType and type(raw) is classmethod
    assert Derived.slot.__self__ is Derived and Derived.slot.__func__ is object.__str__
    add = MethodType(int.__add__, 4)
    assert add.__func__ is int.__add__ and add(3) == 7
    assert not hasattr((4).__add__, "__func__")
    try:
        add.missing
    except AttributeError as exc:
        assert str(exc) == "'wrapper_descriptor' object has no attribute 'missing'"
    else:
        raise AssertionError("method delegated to the wrong callable")
    for name, value in (("__func__", int.__add__), ("__self__", 4)):
        member = vars(MethodType)[name]
        assert member is getattr(MethodType, name)
        assert member.__get__(add) is value
        try:
            setattr(bound, name, value)
        except AttributeError as exc:
            assert str(exc) == "readonly attribute"
        else:
            raise AssertionError(name)
    assert MethodType.__doc__ == "Create a bound instance method object."
    assert classmethod.__doc__.startswith("Convert a function to be a class method.")
    assert bound.__self__ is Derived and bound.__func__ is raw.__func__
    assert str(signature(bound)) == "()" and str(signature(raw.__func__)) == "(cls)"
    assert raw.__doc__ == bound.__doc__ == "Return the class used to bind this method."
    raw.__doc__ = "Descriptor override"
    assert raw.__doc__ == "Descriptor override" and bound.__doc__ == bound.__func__.__doc__
    for builtin_type in (MethodType, classmethod):
        try:
            builtin_type.__doc__ = "Type override"
        except TypeError:
            pass
        else:
            raise AssertionError("writable builtin type docstring")
    assert vars(raw)["__name__"] == "explicit"
    assert Derived[str] == (Derived, str)
    outer = Base.__dict__["outer"]
    inner = Base.__dict__["inner"]
    assert outer.__final__ is True and not hasattr(outer.__func__, "__final__")
    assert not hasattr(inner, "__final__") and inner.__func__.__final__ is True
    assert Child.outer() is Child and Child().inner() is Child
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
