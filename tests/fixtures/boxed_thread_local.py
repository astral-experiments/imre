"""Exercise the persistent, per-instance attribute dictionary used by Click."""

from _thread import _local
from threading import local


def current():
    state = local()
    unused = _local()
    backing = state.__dict__
    print(local is _local, type(state) is local, isinstance(unused, local))
    print(local.__module__, local.__name__, backing is state.__dict__)
    state.stack = []
    backing.setdefault("stack", []).append("outer")
    state.stack.append("inner")
    print(state.stack.pop(), backing, unused.__dict__)
    del state.stack
    backing["__class__"] = "shadow"
    print(state.__class__, getattr(state, "__class__"), type(state) is local)
    print(object.__getattribute__(state, "__class__") is local)
    del backing["__class__"]
    print(state.__class__ is local)
    state.item = 7
    for action in (
        lambda: object.__getattribute__(state, "item"),
        lambda: object.__getattribute__(state, "__dict__"),
        lambda: object.__setattr__(state, "item", 8),
        lambda: object.__delattr__(state, "item"),
        lambda: state.stack,
        lambda: delattr(state, "missing"),
        lambda: setattr(state, "__dict__", {}),
        lambda: delattr(state, "__dict__"),
        lambda: local(1),
        lambda: local(value=1),
    ):
        try:
            action()
        except (AttributeError, TypeError) as error:
            print(type(error).__name__, str(error))
    print(state.item, backing is state.__dict__)
    attribute_names()
    slot_methods()
    descriptor_lookup()


def attribute_names():
    class Key(str):
        def __eq__(self, other):
            print("compared", other)
            return True

        def __repr__(self):
            return "<original key>"

    key = Key("item")
    state = local()
    state.item = 7
    print(getattr(state, key) is state.__dict__, hasattr(state, key))
    for action in (lambda: setattr(state, key, 8), lambda: delattr(state, key)):
        try:
            action()
        except AttributeError as error:
            print(type(error).__name__, str(error), state.item)

    class Hooks:
        def __getattribute__(self, name):
            return name is key

        def __setattr__(self, name, value):
            print("set", name is key)

        def __delattr__(self, name):
            print("delete", name is key)

    hooks = Hooks()
    print(getattr(hooks, key))
    setattr(hooks, key, 8)
    delattr(hooks, key)

    class FallbackName(str):
        pass

    fallback_name = FallbackName("missing")

    class Fallback:
        def __getattr__(self, name):
            return name is fallback_name

    print("fallback", getattr(Fallback(), fallback_name))

    class RaisingComparison(str):
        def __eq__(self, other):
            raise LookupError("name comparison")

    class RaisingRepr(Key):
        def __repr__(self):
            raise LookupError("name repr")

    bad = RaisingComparison("item")
    for action in (lambda: getattr(state, bad), lambda: hasattr(state, bad),
                   lambda: setattr(state, bad, 8), lambda: delattr(state, bad),
                   lambda: setattr(state, RaisingRepr("item"), 8)):
        try:
            action()
        except LookupError as error:
            print(error.args, state.item)


def slot_methods():
    state = local()
    state.__setattr__("item", 7)
    get = state.__getattribute__
    state.__getattribute__ = "shadow"
    print(state.__getattribute__, get("item"))
    print(object.__getattribute__(state, "__getattribute__")("item"))
    local.__delattr__(state, "item")
    print(hasattr(state, "item"))

    descriptor = local.__getattribute__
    state.__dict__[17] = "raw"
    print(get(17), descriptor.__get__(None, local) is descriptor, get.__self__ is state)
    for action in (
        lambda: get([]),
        lambda: get(([],)),
        lambda: state.__setattr__([], 3),
        lambda: descriptor.__get__(0),
        lambda: descriptor(),
        lambda: descriptor(state, name="item"),
    ):
        try:
            action()
        except TypeError as error:
            print(error.args)

    original = KeyError("from hash")

    class Interrupted(str):
        def __hash__(self):
            raise original

    key = Interrupted("item")
    for action in (lambda: get(key), lambda: local.__delattr__(state, key),
                   lambda: local.__setattr__(state, key, 8)):
        try:
            action()
        except (KeyError, AttributeError) as error:
            print(type(error).__name__, error is original, error.args)

    state.kept = 43
    del state
    # Let the boxed heap trace with only the saved method keeping the receiver alive.
    for index in range(8192):
        temporary = [index]
    print(get("kept"))


def descriptor_lookup():
    state = local()
    backing = state.__dict__
    calls = [0]
    mode = ["fill"]

    class Key(str):
        def __eq__(self, other):
            return self is other

        def __hash__(self):
            calls[0] += 1
            if calls[0] == 2 and mode[0] == "refuse":
                raise LookupError("second hash")
            if calls[0] == 3 and mode[0] == "fill":
                backing[self] = 37
            return 29

    key = Key("field")
    print(getattr(state, key), calls[0])
    mode[0] = "refuse"
    for change in (lambda: setattr(state, key, 12), lambda: delattr(state, key)):
        calls[0] = 0
        try:
            change()
        except LookupError as error:
            print(str(error), calls[0], backing[key])


if __name__ == "__main__":
    current()
