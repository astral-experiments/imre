import gc
import weakref
from threading import local


class State(local):
    __slots__ = ("shared",)
    flag = 12

    @property
    def count(self):
        return self._count + 1

    @count.setter
    def count(self, value):
        self._count = value

    def report(self):
        return self.flag, self.count


class Derived(State):
    pass


def current():
    state = Derived()
    other = Derived()
    state.flag = 17
    print(type(state) is Derived, isinstance(state, local), other.flag, state.flag)
    backing = state.__dict__
    state.count = 5
    backing["count"] = 77
    print(state.report(), backing["count"])
    state.report = 8
    print(state.report, State.report(state))
    state.shared = "slot"
    backing["shared"] = "shadow"
    print(state.shared, backing["shared"])
    backing["__class__"] = "shadow"
    print(state.__class__ is Derived, state.__dict__ is backing)
    object.__setattr__(state, "hidden", 7)
    print(object.__getattribute__(state, "__dict__")["hidden"], hasattr(state, "hidden"))
    delattr(state, "flag")
    print(state.flag, other.__dict__)

    class Initialized(local):
        def __init__(self, marker, *, token):
            assert not self.__dict__
            self.values = (marker, token)

    class Child(Initialized):
        pass

    marker = object()
    active = Child(marker, token=marker)
    assert active.values[0] is active.values[1] is marker
    class Token:
        pass

    first, second = Token(), Token()
    first_ref, second_ref = weakref.ref(first), weakref.ref(second)
    raw = local.__new__(Child, first, token=second)
    assert raw.__dict__ == {}
    del first, second
    gc.collect()
    assert first_ref() is not None and second_ref() is not None
    del raw
    gc.collect()
    assert first_ref() is None and second_ref() is None
