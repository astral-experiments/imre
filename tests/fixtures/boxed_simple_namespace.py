"""Exercise the boxed SimpleNamespace descriptor used by pprint's dispatch table."""

from types import SimpleNamespace


class Child(SimpleNamespace):
    pass


def current():
    ns = SimpleNamespace([("first", 1), ("last", 2)], first=3)
    descriptor = SimpleNamespace.__repr__
    handlers = {descriptor: descriptor}
    assert handlers[type(ns).__repr__](ns) == "namespace(first=3, last=2)"
    assert descriptor.__objclass__ is SimpleNamespace
    assert Child.__repr__ is descriptor
    assert repr(Child(x=7)) == "Child(x=7)"

    fields = ns.__dict__
    del ns.first
    fields["first"] = ns
    assert ns.first is ns
    assert repr(ns) == "namespace(last=2, first=namespace(...))"

    try:
        SimpleNamespace.__repr__ = str
    except TypeError:
        pass
    else:
        raise AssertionError("the builtin repr slot was writable")

    assert handlers[type(ns).__repr__] is descriptor
    print("passed")
