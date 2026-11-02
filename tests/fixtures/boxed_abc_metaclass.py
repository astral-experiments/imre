import abc
from abc import ABCMeta as Meta


def current():
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
