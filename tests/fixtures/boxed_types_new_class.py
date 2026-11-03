import types
from types import get_original_bases


def helpers():
    events = []

    class Expand:
        def __mro_entries__(self, bases):
            events.append(("resolve", bases))
            return (int, str)

    class Factory:
        def __prepare__(self, name, bases, **kwds):
            events.append(("prepare", name, bases, kwds))
            return {"prepared": True}

        def __call__(self, name, bases, namespace, **kwds):
            events.append(("factory", name, bases, kwds))
            return namespace

    def body(namespace):
        assert "__orig_bases__" not in namespace
        events.append(("body", namespace["prepared"]))
        namespace["payload"] = 42

    factory = Factory()
    kwds = {"metaclass": factory, "flag": 7}
    original = (Expand(),)
    namespace = types.new_class("Example", original, kwds, body)
    assert kwds == {"metaclass": factory, "flag": 7}
    assert namespace == {"prepared": True, "payload": 42, "__orig_bases__": original}
    assert events == [
        ("resolve", original), ("prepare", "Example", (int, str), {"flag": 7}),
        ("body", True), ("factory", "Example", (int, str), {"flag": 7}),
    ]
    bases = (int, str)
    assert types.resolve_bases(bases) is bases
    assert types.prepare_class("Example") == (type, {}, {})

    class Bad:
        def __mro_entries__(self, bases):
            return [int]

    events.clear()
    try:
        types.new_class("Bad", (Bad(),), kwds, body)
    except TypeError as exc:
        assert str(exc) == "__mro_entries__ must return a tuple"
    else:
        raise AssertionError("accepted invalid MRO entries")
    assert not events

    def failing_body(namespace):
        raise LookupError("body")

    try:
        types.new_class("Failed", (), kwds, failing_body)
    except LookupError as exc:
        assert exc.args == ("body",)
    else:
        raise AssertionError("class body error disappeared")
    assert events == [("prepare", "Failed", (), {"flag": 7})]


def metaclass_preparation():
    # The native type slot must take precedence even on the first lookup.
    class Later:
        @classmethod
        def __prepare__(cls, name, bases):
            raise AssertionError("looked past type in metaclass MRO")

    class Ordered(type, Later): pass

    assert types.prepare_class("Probe", (), {"metaclass": Ordered}) == (Ordered, {}, {})

    class Plain: pass
    class Default(type): pass
    class Child(metaclass=Default): pass

    assert type(Child) is Default
    assert Plain.__prepare__.__self__ is type
    assert Default.__prepare__.__self__ is Default
    assert Child.__prepare__.__self__ is Default
    assert super(Default, Default).__prepare__.__self__ is Default
    assert not hasattr(super(Plain, Plain), "__prepare__")
    first = type.__prepare__()
    assert first == {} and type.__prepare__("N", (), None, extra=True) is not first

    events = []

    class Meta(type):
        @classmethod
        def __prepare__(mcls, name, bases, **kw):
            ns = super().__prepare__(name, bases, **kw)
            events.append(("prepare", ns))
            return ns

        def __new__(mcls, name, bases, ns, **kw):
            events.append(("new", ns))
            return super().__new__(mcls, name, bases, ns)

    def body(ns):
        events.append(("body", ns))
        ns["answer"] = 42

    created = types.new_class("Created", (), {"metaclass": Meta, "tag": True}, body)
    assert type(created) is Meta and created.answer == 42
    assert [name for name, ns in events] == ["prepare", "body", "new"]
    assert events[0][1] is events[1][1] is events[2][1]

    class Disabled(type):
        __prepare__ = None

    try:
        types.new_class("Invalid", (), {"metaclass": Disabled}, body)
    except TypeError:
        pass
    else:
        raise AssertionError("called body after a non-callable preparation hook")
    assert len(events) == 3


def current():
    metaclass_preparation()
    helpers()

    class First:
        def first(self):
            return 1

    class Second:
        def second(self):
            return 2

    def fill(namespace):
        namespace["answer"] = 42

    result = types.new_class("Result", (First, Second), exec_body=fill)
    assert result.__bases__ == (First, Second)
    assert result().answer == 42 and result().first() + result().second() == 3
    assert types.new_class("Empty").__module__ == "types"


    class Sequence(list[int]):
        pass

    class Child(Sequence):
        pass

    assert get_original_bases(Sequence) == (list[int],)
    assert get_original_bases(Child) == (Sequence,)
    assert get_original_bases(int) == (object,)
    assert get_original_bases(bool) == (int,)
    try:
        get_original_bases(object())
    except TypeError as exc:
        assert str(exc) == "Expected an instance of type, not 'object'"
    else:
        raise AssertionError("accepted a non-class")
