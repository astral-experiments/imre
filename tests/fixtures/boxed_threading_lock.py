import _thread
import os
import types
import threading
from _thread import LockType, error as lock_error
from threading import Lock, TIMEOUT_MAX, ThreadError
from weakref import ref


class MapLock:
    lock_class = Lock


def factories():
    assert "Lock" in dir(threading)
    assert threading.Lock is LockType
    try:
        del threading.Lock
        assert "Lock" not in dir(threading)
        assert not hasattr(threading, "Lock")
    finally:
        threading.Lock = Lock
    assert ThreadError is lock_error is _thread.error is RuntimeError
    try:
        Lock().release()
    except ThreadError as error:
        assert type(error) is RuntimeError
    else:
        raise AssertionError("released an unlocked lock")
    assert TIMEOUT_MAX is _thread.TIMEOUT_MAX
    print(TIMEOUT_MAX)
    assert _thread.allocate is not _thread.allocate_lock
    assert _thread.allocate == _thread.allocate_lock
    assert {_thread.allocate: 1}[_thread.allocate_lock] == 1
    for factory in (_thread.allocate, _thread.allocate_lock):
        assert type(factory) is types.BuiltinFunctionType
        assert factory.__self__ is _thread
        assert factory is _thread.__dict__[factory.__name__]
        print(repr(factory), factory.__name__, factory.__qualname__, factory.__module__)
        print(factory.__text_signature__, factory.__doc__)
        for args, kwargs in [((1,), {}), ((), {"x": 1}), ((1,), {"x": 1})]:
            try:
                factory(*args, **kwargs)
            except TypeError as error:
                print(str(error))
            else:
                raise AssertionError("lock factory accepted arguments")
        left, right = factory(), factory()
        assert type(left) is type(right) is LockType
        assert left.acquire(timeout=TIMEOUT_MAX) and right.acquire(False)
        assert not left.acquire(False)
        left.release()
        right.release()
        try:
            left.acquire(timeout=TIMEOUT_MAX + 1)
        except OverflowError:
            pass
        else:
            raise AssertionError("lock accepted timeout above TIMEOUT_MAX")


def current():
    try:
        class DerivedLock(Lock):
            pass
    except TypeError as error:
        assert str(error) == "type '_thread.lock' is not an acceptable base type"
    else:
        raise AssertionError("primitive lock accepted a subclass")

    factories()
    lock = MapLock().lock_class()
    print(type(lock) is Lock is LockType, isinstance(lock, Lock), Lock.__module__, Lock.__name__)
    print(lock.locked(), lock.acquire(False), lock.acquire(blocking=False), lock.acquire(timeout=0))
    print(Lock.release(lock), lock.locked())
    assert repr(lock) == f"<unlocked _thread.lock object at {id(lock):#x}>"
    with lock as entered:
        assert repr(lock) == f"<locked _thread.lock object at {id(lock):#x}>"
        print(entered, lock.locked(), lock.acquire(False))
    try:
        with lock:
            raise ValueError("body")
    except ValueError as error:
        print(str(error), lock.locked())
    for action in (
        lambda: lock.release(),
        lambda: lock.acquire(False, 1),
        lambda: lock.acquire(timeout=-0.5),
        lambda: lock.acquire(timeout=1e100),
        lambda: lock.acquire(timeout=10**50),
        lambda: lock.acquire(timeout=None),
        lambda: lock.acquire(False, blocking=False),
        lambda: Lock(1),
    ):
        try:
            action()
        except (RuntimeError, ValueError, TypeError, OverflowError) as error:
            print(type(error).__name__, str(error))
    print(lock.acquire(timeout=0.001), lock.acquire(timeout=0.001), lock.release())

    observed = ref(lock)
    assert observed() is lock
    del lock
    assert observed() is None


def reinit_during_acquire():
    inherited = Lock()
    inherited.acquire()
    acquire = inherited.acquire

    class ResetTimeout:
        def __index__(self):
            inherited._at_fork_reinit()
            return 0

    pid = os.fork()
    if pid == 0:
        os._exit(0 if acquire(timeout=ResetTimeout()) and inherited.locked() else 1)
    _, status = os.waitpid(pid, 0)
    assert status == 0 and inherited.locked()
    inherited.release()


if __name__ == "__main__":
    current()
    if hasattr(os, "fork"):
        reinit_during_acquire()
