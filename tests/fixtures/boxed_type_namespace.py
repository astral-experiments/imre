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

    class DefaultMeta(type):
        pass
    assert DefaultMeta.__init__ is type.__init__

    calls = []
    class InitializingMeta(DefaultMeta):
        def __init__(cls, name, bases, body, **kw):
            super().__init__(name, bases, body, **kw)
            calls.append((cls, name, bases, body, kw))
            cls.generated = name.lower()

    supplied = {"marker": object()}
    parent = InitializingMeta("Parent", (Base,), supplied, label=1)
    class Child(parent, label=2):
        saved_body = locals()

    dynamic = type("Dynamic", (Child,), {}, label=3)
    assert [event[0] for event in calls] == [parent, Child, dynamic]
    assert calls[0][3] is supplied and calls[1][3] is Child.saved_body
    assert [event[4]["label"] for event in calls] == [1, 2, 3]
    assert calls[1][2] == (parent,) and type(dynamic) is InitializingMeta
    assert dynamic.generated == "dynamic" and parent.marker is supplied["marker"]
    assert "generated" not in supplied and "generated" not in Child.saved_body

    from abc import ABCMeta
    class Redirect(ABCMeta):
        def __new__(meta, name, bases, body):
            if name == "Redirected":
                return parent
            return ABCMeta.__new__(meta, name, bases, body)

    class RedirectBase(metaclass=Redirect):
        pass

    calls.clear()
    body = {}
    assert type("Redirected", (RedirectBase,), body) is parent
    assert Redirect("Redirected", (RedirectBase,), body) is parent
    assert calls == [(parent, "Redirected", (RedirectBase,), body, {})]
    assert calls[0][3] is body

    calls.clear()
    created = []
    class Delegating(InitializingMeta):
        def __new__(meta, name, bases, namespace):
            created.append(name)
            return super().__new__(meta, name, bases, namespace)

    raw = type.__new__(Delegating, "Raw", (), {})
    assert type(raw) is Delegating and not calls and not created
    combined = Delegating("Combined", (), body)
    assert created == ["Combined"] and calls == [(combined, "Combined", (), body, {})]

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
