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


class WeakTarget:
    pass


def clear_weak_target():
    targets = [WeakTarget()]
    witness = ref(targets[0], weak_callback)
    assert (targets.clear(), events.copy()) == (None, ["weak"])
    return witness


def check_arguments(owner, witness):
    assert isinstance(owner, Owner) and witness() is None
    assert events == ["weak"]


def current():
    # Keep another pending finalizer reachable while cyclic and acyclic owners die.
    held = [Owner()]
    witness = ref(held[0])
    exercise(True, False)
    exercise(True, True)
    exercise(False, False)
    assert witness() is held[0]
    held.clear()
    assert events == ["finalize"] and witness() is saved[0]
    saved.clear()
    assert witness() is None
    events.clear()
    # The first argument stays live while the second releases an unrelated target.
    check_arguments(Owner(), clear_weak_target())
    assert events == ["weak", "finalize"]
    saved.clear()
    events.clear()
