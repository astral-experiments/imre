"""Exercise the boxed SimpleNamespace descriptor used by pprint's dispatch table."""

from copy import replace
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
    class Initialized(SimpleNamespace):
        def __init__(self):
            self.default = "new"

    original = Initialized()
    del original.default
    original.items = []
    vars(original)[1] = "non-string key"
    replaced = replace(original, changed=True)
    assert type(replaced) is Initialized
    assert vars(replaced) == {"default": "new", "items": [], 1: "non-string key", "changed": True}
    assert replaced.items is original.items
    assert not hasattr(original, "changed")
    descriptor = SimpleNamespace.__replace__
    assert descriptor.__objclass__ is SimpleNamespace
    assert descriptor.__text_signature__ == "($self, /, **changes)"
    assert descriptor.__doc__ == "Return a copy of the namespace object with new values for the specified attributes."
    try:
        original.__replace__(1)
    except TypeError as error:
        assert str(error) == "__replace__() takes no positional arguments"
    else:
        raise AssertionError("accepted a positional change")
    print("passed")
