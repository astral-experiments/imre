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

    class Custom(type):
        pass

    try:
        class Unsupported(metaclass=Custom):
            events.append("custom")
    except NotImplementedError:
        assert events == ["meta", "body"]
    else:
        assert type(Unsupported) is Custom and events[-1] == "custom"
