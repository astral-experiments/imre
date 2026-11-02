"""A detached callback keeps the original Python arguments alive until it exits."""
from _thread import LockType, _count, start_new_thread
from time import monotonic
from weakref import ref


class Args(tuple):
    pass


class Kwargs(dict):
    pass


def target():
    def handoff(ready, proceed):
        ready.release()
        proceed.acquire()
    return handoff


def run():
    ready, proceed = LockType(), LockType()
    ready.acquire()
    proceed.acquire()
    callback = target()
    weak_callback = ref(callback)
    start_new_thread(callback, Args((ready,)), Kwargs(proceed=proceed))
    del callback
    assert ready.acquire(timeout=5)
    assert _count() == 1 and weak_callback() is not None
    proceed.release()
    deadline = monotonic() + 5
    while _count() and monotonic() < deadline:
        pass
    assert _count() == 0
    assert weak_callback() is None


run()
