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
