from collections import OrderedDict


def make_reader():
    captured = object()
    def read():
        return captured
    return read


def current():
    events = []

    class Base:
        def __init_subclass__(cls, *, label):
            events.append(("subclass", cls, label))

    class Mixin:
        def inherited(self):
            return "inherited"

    class Field:
        def __set_name__(self, owner, name):
            events.append((name, owner))
            namespace.clear()

    namespace = {"first": Field(), "second": Field(), "__qualname__": "Outer.Child"}
    child = type("Child", (Base, Mixin), namespace, label=7)
    assert namespace == {}
    assert child.__module__ == __name__ and child.__qualname__ == "Outer.Child"
    assert "__qualname__" not in child.__dict__
    assert child.__mro__ == (child, Base, Mixin, object)
    assert child().inherited() == "inherited"
    assert events == [("first", child), ("second", child), ("subclass", child, 7)]

    reader = make_reader()
    cell = reader.__closure__[0]
    rebound = type("Rebound", (), {"__classcell__": cell})
    assert reader() is rebound and cell.cell_contents is rebound
    assert "__classcell__" not in rebound.__dict__

    calls = []
    class Record:
        def __set_name__(self, owner, name):
            calls.append(name)

    key = "non-identifier key!".encode().decode()
    fields = OrderedDict([(key, Record()), ("second", Record())])
    fields.move_to_end(key)
    with_fields = type("WithFields", (), fields)
    assert calls == ["second", key] and calls[-1] is key
    assert next(k for k in with_fields.__dict__ if k == key) is key

    class Static:
        @staticmethod
        def __set_name__(owner, name):
            calls.append((owner, name))

    class Bound:
        @classmethod
        def __set_name__(cls, owner, name):
            calls.append((cls, owner, name))

    hooked = type("Hooked", (), {"static": Static(), "bound": Bound()})
    assert calls[-2:] == [(hooked, "static"), (Bound, hooked, "bound")]

    holder = make_reader()
    live = type("Live", (), {"__classdictcell__": holder.__closure__[0]})
    assert "__classdictcell__" not in live.__dict__
    live.updated = "from class"
    assert holder()["updated"] == "from class"
    holder()["new_field"] = "from dictionary"
    assert live.new_field == "from dictionary"

    unhashable = type("Unhashable", (), {"__eq__": lambda self, other: self is other})
    assert unhashable.__dict__["__hash__"] is None
    try:
        hash(unhashable())
    except TypeError:
        pass
    else:
        raise AssertionError("unhashable class")

    for bases in ((bool,), (int, str)):
        try:
            type("invalid\0", bases, {})
        except TypeError:
            pass
        else:
            raise AssertionError(bases)

    for bases, namespace in (((list, dict), {}), ((), {"__classdictcell__": 4}),
                             ((), {"__slots__": ("__dict__", "__dict__")}),
                             ((Mixin,), {"__slots__": ("__dict__",)}),
                             ((tuple,), {"__slots__": ("field",)})):
        try:
            type("Invalid", bases, namespace)
        except TypeError:
            pass
        else:
            raise AssertionError(bases)

    for base in (bool, range, memoryview, type(reader), type(iter([])), type({}.keys())):
        try:
            type("Invalid", (base,), {})
        except TypeError as exc:
            assert "not an acceptable base type" in str(exc)
        else:
            raise AssertionError(base)
