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
    for action in (
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


if __name__ == "__main__":
    current()
