import gc
from weakref import ref


class Parent:
    def __del__(self):
        events.append("finalize")
        saved.append(self)


class Owner(Parent):
    pass


saved = []
events = []


def weak_callback(w):
    assert w() is None
    events.append("weak")


def exercise(cyclic, unrelated):
    gc.collect(2)
    owner = Owner()
    owner.payload = []
    if cyclic:
        owner.loop = owner
    early = ref(owner, weak_callback)
    del owner

    if unrelated:
        extra = []
        extra.append(extra)
        del extra

    if cyclic:
        assert early() is not None and events == []
    else:
        assert early() is saved[0] and events == ["finalize"]
    # A resurrected owner and its payload do not contribute to this count.
    assert gc.collect(2) == int(unrelated)
    if cyclic:
        assert early() is None and events == ["weak", "finalize"]
    else:
        assert early() is saved[0] and events == ["finalize"]

    late = ref(saved[0], weak_callback)
    saved.clear()
    if cyclic:
        assert late() is not None and gc.collect(2) == 2
        assert events == ["weak", "finalize", "weak"]
    else:
        assert late() is None and events == ["finalize", "weak", "weak"]
    assert early() is None and late() is None and gc.collect(2) == 0
    events.clear()


def current():
    exercise(True, False)
    exercise(True, True)
    exercise(False, False)
