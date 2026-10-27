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


if __name__ == "__main__":
    current()
