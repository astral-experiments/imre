import gc
import sys
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


class Receiver:
    def check(self, witness):
        assert witness() is None and self.owner is None
        assert events == ["weak", "finalize"], events


def bound_method(receivers):
    receiver = Receiver()
    receiver.owner = Owner()
    receivers.append(ref(receiver))
    return receiver.check


def detach_owner(receivers, depth):
    if depth:
        return detach_owner(receivers, depth - 1)
    witness = clear_weak_target()
    receivers[0]().owner = None
    assert events == ["weak", "finalize"], events
    return witness


def check_unraisable():
    failures = []
    problem = ValueError("in the destructor")

    class Rejected:
        def __del__(self):
            raise problem

    before = sys.unraisablehook
    try:
        sys.unraisablehook = failures.append
        obj = Rejected()
        del obj
        gc.collect()
        event, = failures
        assert type(type(event)) is type and isinstance(event, tuple)
        assert event.exc_type is ValueError and event.exc_value is problem
        assert event.exc_traceback is problem.__traceback__
        assert event.object is None
        assert event.err_msg == f"Exception ignored while calling deallocator {Rejected.__del__!r}"
        assert tuple(event) == (ValueError, problem, event.exc_traceback, event.err_msg, None)
    finally:
        sys.unraisablehook = before


def check_replacement_line():
    observed = []

    class Resource:
        def __del__(self):
            frame = sys._getframe(1)
            observed.append((frame.f_code.co_name, frame.f_lineno))

    def release(annotated):
        handle = Resource()
        alias = handle
        handle = None
        assert len(observed) == len(expected)
        if annotated:
            expected.append(sys._getframe().f_lineno + 1)
            alias: object = None
        else:
            expected.append(sys._getframe().f_lineno + 1)
            alias = None

    expected = []
    release(False)
    release(True)
    assert observed == [("release", line) for line in expected], observed


def check_exception_finalizers():
    finalized = []

    class BaseError(Exception):
        pass

    class ChildError(BaseError):
        pass

    before = ChildError("before")

    def finalize(error):
        finalized.append(error.args[0])

    BaseError.__del__ = finalize
    after = ChildError("after")
    gc.collect()
    assert finalized == []
    del before
    gc.collect()
    assert finalized == ["before"]
    del after
    gc.collect()
    assert finalized == ["before", "after"]


def current():
    check_exception_finalizers()
    check_replacement_line()
    check_unraisable()
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
    # The receiver stays live as a bound method while its owner is detached.
    receivers = []
    bound_method(receivers)(detach_owner(receivers, 3))
    assert receivers[0]() is None and events == ["weak", "finalize"]
    saved.clear()
    events.clear()
    # Nested fields keep a finalizer alive until its last strong path changes.
    outer = Receiver()
    outer.inner = Receiver()
    outer.inner.owner = Owner()
    alias = outer.inner
    assert (setattr(outer, "inner", None), events.copy()) == (None, [])
    del alias
    assert events == ["finalize"]
    saved.clear()
    events.clear()
    # Reusing the same field must not preserve the old reachability result.
    outer.inner = Receiver()
    outer.inner.owner = Owner()
    assert (setattr(outer, "inner", None), events.copy()) == (None, ["finalize"])
    saved.clear()
    events.clear()
