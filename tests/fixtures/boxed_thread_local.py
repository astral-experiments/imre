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


if __name__ == "__main__":
    current()
