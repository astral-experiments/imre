import abc
from abc import ABCMeta as Meta


def current():
    from collections.abc import ByteString
    import warnings

    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        assert issubclass(bytes, ByteString) and issubclass(bytearray, ByteString)
        assert not issubclass(memoryview, ByteString)
        try:
            isinstance(b"", ByteString)
        except DeprecationWarning as warning:
            assert "collections.abc.ByteString" in str(warning)
        else:
            raise AssertionError("ByteString skipped its metaclass warning")

    checked = []
    class CheckMeta(Meta):
        def __subclasscheck__(cls, child):
            checked.append(child)
            return super().__subclasscheck__(child)

        def __instancecheck__(cls, obj):
            seen.append(obj)
            return super().__instancecheck__(obj)

    class Target(metaclass=CheckMeta):
        annotation_type = int
        field: annotation_type

    Target.annotation_type = str
    assert Target.__annotations__ == {"field": str}
    seen = []
    exact = Target()
    assert isinstance(exact, Target) and not seen

    class ClaimsTarget:
        @property
        def __class__(self):
            return Target

    assert not issubclass(ClaimsTarget, Target) and checked == [ClaimsTarget]
    assert Meta.__subclasscheck__(Target, Target)

    claim = ClaimsTarget()
    assert isinstance(claim, Target) and seen == [claim]

    class ActualChild(Target):
        @property
        def __class__(self):
            return str

    child = ActualChild()
    assert isinstance(child, Target) and seen == [claim, child]

    events = []

    def metaclass():
        events.append("meta")
        return Meta

    class Abstract(metaclass=metaclass()):
        events.append("body")

        @abc.abstractmethod
        def read(self):
            pass

    assert events == ["meta", "body"]
    assert type(abc.ABC) is type(Abstract) is Meta
    assert isinstance(Abstract, Meta)
    assert Abstract.__bases__ == (object,)
    assert not issubclass(Abstract, abc.ABC)
    assert Abstract.__abstractmethods__ == frozenset({"read"})

    try:
        abc.ABCMeta = type

        class Incomplete(Abstract):
            pass

        class Complete(Incomplete):
            def read(self):
                return b"io"

        for cls in (Abstract, Incomplete):
            assert type(cls) is Meta
            try:
                cls()
            except TypeError as exc:
                assert str(exc) == f"Can't instantiate abstract class {cls.__name__} without an implementation for abstract method 'read'"
            else:
                raise AssertionError("constructed an abstract class")
        assert type(Complete) is Meta and Complete.__abstractmethods__ == frozenset()
        assert Complete().read() == b"io"
    finally:
        abc.ABCMeta = Meta

    namespace = {"Meta": Meta}
    exec("class Virtual(metaclass=Meta): marker = 42", namespace)
    virtual = namespace["Virtual"]
    assert type(virtual) is Meta and virtual.marker == 42
    token = abc.get_cache_token()
    assert Abstract.register(virtual) is virtual
    changed = abc.get_cache_token()
    assert changed != token
    assert Abstract.register(virtual) is virtual
    assert abc.get_cache_token() == changed
    assert Abstract.register(Complete) is Complete
    assert abc.get_cache_token() == changed
    assert issubclass(virtual, Abstract) and isinstance(virtual(), Abstract)
    assert Abstract not in virtual.__mro__

    class Factory(type):
        __call__ = dict

    class Refined(Factory):
        def __call__(cls, *, key):
            return cls, key

    class Base(metaclass=Factory):
        pass

    class Derived(metaclass=Refined):
        pass

    class Combined(Base, Derived):
        pass

    assert type(Base) is Factory and type(Combined) is Refined
    assert Base(key=42) == {"key": 42}
    assert Combined(key=42) == (Combined, 42)

    class Other(type):
        pass

    try:
        class Conflict(Base, metaclass=Other):
            events.append("conflict")
    except TypeError as exc:
        assert "metaclass conflict" in str(exc)
    else:
        raise AssertionError("accepted incompatible metaclasses")
    assert events == ["meta", "body"]

    try:
        direct = Factory("Direct", (), {})
    except NotImplementedError:
        pass
    else:
        assert type(direct) is Factory and direct(key=3) == {"key": 3}

    Factory.__call__ = Base
    try:
        Base()
    except RecursionError:
        pass
    else:
        raise AssertionError("recursive metaclass call returned")


    check_namespace_constructor()


def check_namespace_constructor():
    def make_cell(value):
        return (lambda: value).__closure__[0]

    cell = make_cell("previous")
    dict_cell = make_cell(None)
    events = []

    class Member:
        def __set_name__(self, owner, key):
            assert cell.cell_contents is owner
            assert dict_cell.cell_contents[key] is self
            assert "__classcell__" not in owner.__dict__
            assert "__classdictcell__" not in owner.__dict__
            events.append(key)

    class Base(metaclass=Meta):
        def __init_subclass__(cls, *, mark):
            events.append(mark)

    ns = {"a": Member(), "__classcell__": cell, "b": Member(), "__classdictcell__": dict_cell}
    child = Meta.__new__(Meta, "Child", (Base,), ns, mark="hook")
    assert events == ["a", "b", "hook"]
    assert cell.cell_contents is child and child.__module__ == "abc"
    assert child.a is ns["a"] and child.b is ns["b"]
    ns["a"] = None
    assert child.a is not None

    try:
        Meta.__new__(Meta, "Bad", (), {"x": Member(), "__classcell__": None})
    except TypeError as error:
        assert "__classcell__ must be a nonlocal cell" in str(error)
    else:
        raise AssertionError("invalid class cell reached a descriptor callback")
    assert events == ["a", "b", "hook"]
