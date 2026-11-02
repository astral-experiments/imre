import os
from _thread import RLock, allocate_lock, start_new_thread
from functools import RLock as FunctoolsRLock
from weakref import ref


class Tagged(RLock):
    def __init__(self, tag):
        super().__init__()
        self.tag = tag


class Timeout(float):
    def __index__(self):
        raise AssertionError("used __index__ on a float timeout")


class SavedOwner:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        if self.value is None:
            raise ValueError("invalid saved owner")
        return self.value


def acquire_on_worker(lock, again, started, finished):
    if not lock.acquire(False):
        started.release()
    lock.acquire()
    if again.acquire(False):
        again.release()
        lock.release()
        finished.release()


def current():
    lock = RLock()
    assert type(lock) is RLock and not lock.locked()
    assert lock.acquire(timeout=Timeout(0.5))
    lock.release()
    with lock as entered:
        assert entered is True and lock.acquire(False)
        assert lock._is_owned() and lock._recursion_count() == 2
        state = lock._release_save()
        assert not lock.locked() and not lock._is_owned() and lock._recursion_count() == 0
        assert state[0] == 2 and type(state[1]) is int
        try:
            lock._acquire_restore((state[0], SavedOwner(None)))
        except ValueError as error:
            assert str(error) == "invalid saved owner"
        else:
            raise AssertionError("ignored the saved owner's index error")
        assert not lock.locked()
        lock._acquire_restore((state[0], SavedOwner(state[1])))
        assert lock._is_owned() and lock._release_save() == state
        assert lock._acquire_restore(state) is None and lock._recursion_count() == 2
        assert repr(lock) == f"<locked _thread.RLock object owner={state[1]} count=2 at {id(lock):#x}>"
        lock.release()
    assert repr(lock) == f"<unlocked _thread.RLock object owner=0 count=0 at {id(lock):#x}>"
    try:
        lock.release()
    except RuntimeError as error:
        assert str(error) == "cannot release un-acquired lock"
    else:
        raise AssertionError("released an unowned RLock")
    try:
        with lock:
            lock.release(1)
    except TypeError as error:
        assert str(error) == "RLock.release() takes no arguments (1 given)"
        assert not lock.locked()
    else:
        raise AssertionError("invalid release did not escape the RLock context")
    observed = ref(lock)
    assert observed() is lock
    del lock
    assert observed() is None

    tagged = Tagged("tag")
    assert tagged.tag == "tag" and isinstance(tagged, RLock)
    assert repr(tagged) == f"<unlocked Tagged object owner=0 count=0 at {id(tagged):#x}>"
    with tagged:
        assert tagged._recursion_count() == 1 and tagged.locked()
    assert FunctoolsRLock is RLock
    shared = FunctoolsRLock()
    started = allocate_lock()
    finished = allocate_lock()
    started.acquire()
    finished.acquire()
    with shared:
        start_new_thread(acquire_on_worker, (shared, shared, started, finished))
        assert started.acquire(timeout=5) and shared._is_owned()
    assert finished.acquire(timeout=5) and not shared.locked()
    if os.name == "posix":
        tagged.acquire()
        tagged.acquire()
        reset = tagged._at_fork_reinit
        assert reset() is None and tagged._recursion_count() == 0
        assert not tagged.locked() and not tagged._is_owned()
        assert tagged.acquire(False) and tagged._recursion_count() == 1
        tagged.release()
    print("reentrant lock")


if __name__ == "__main__":
    current()
