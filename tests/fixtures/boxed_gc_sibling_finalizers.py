import gc
import sys
import warnings
from weakref import ref


events = []
saved = []


class WarnOwner:
    def __del__(self):
        warnings.warn("released", ResourceWarning, stacklevel=2)


class Child:
    def __del__(self):
        events.append("child")


class Parent:
    def __del__(self):
        events.append("before")
        self.child = None
        events.append("after")


class Peer:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        other = self.sibling()
        if other is not None and not events:
            saved.append(other)
        events.append(self.name)


def release_two():
    first = WarnOwner()
    second = WarnOwner()


def release_with_weakref():
    first = Peer("left")
    second = Peer("right")
    first.sibling = ref(second)
    second.sibling = ref(first)


def current():
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", ResourceWarning)
        line = sys._getframe().f_lineno + 1
        release_two()
    assert [(w.filename, w.lineno) for w in caught] == [(__file__, line)] * 2

    release_with_weakref()
    assert len(saved) == 1 and len(events) == 1
    assert saved[0].name != events[0]
    gc.collect()
    assert len(events) == 1
    saved.clear()
    gc.collect()
    assert sorted(events) == ["left", "right"]
    events.clear()

    owner = Parent()
    owner.child = Child()
    del owner
    assert events == ["before", "child", "after"]
    events.clear()
