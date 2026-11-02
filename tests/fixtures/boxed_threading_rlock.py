import _thread
import threading
import warnings
from time import monotonic


def factory():
    assert "RLock" in threading.__all__
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert type(threading.RLock()) is _thread.RLock
        assert not caught
        assert type(threading.RLock(1, tag=2)) is _thread.RLock
        assert len(caught) == 1 and caught[0].category is DeprecationWarning
        assert caught[0].filename == __file__
        assert str(caught[0].message) == "Passing arguments to RLock is deprecated and will be removed in 3.15"

    native = threading._CRLock
    try:
        threading._CRLock = None
        assert type(threading.RLock()) is threading._PyRLock
    finally:
        threading._CRLock = native


def hold_block(block, ready, start, entered, proceed, done):
    ready.release()
    start.acquire()
    block.acquire()
    entered.release()
    proceed.acquire()
    block.release()
    done.release()


def recursive():
    assert threading._PyRLock is threading._RLock
    assert getattr(_thread, "RLock", None) is threading._CRLock
    lock = threading._PyRLock()
    assert lock.acquire() is True
    result = lock.acquire(False)
    assert type(result) is int and result == 1
    assert lock._is_owned() and lock._recursion_count() == 2
    state = lock._release_save()
    assert state == (2, _thread.get_ident()) and not lock.locked()
    assert not lock._is_owned() and lock._recursion_count() == 0
    assert lock._acquire_restore(state) is None
    assert lock.release() is None and lock.locked()
    assert lock.release() is None and not lock.locked()
    try:
        with lock as first:
            assert first is True
            with lock as second:
                assert type(second) is int and second == 1
                raise ValueError("body")
    except ValueError as exc:
        assert exc.args == ("body",)
    assert not lock.locked() and lock._recursion_count() == 0

    alias = lock._block
    locked = alias.locked
    lock.acquire()
    lock.acquire()
    assert lock._at_fork_reinit() is None
    assert alias is lock._block and not locked()
    assert not lock._is_owned() and lock._recursion_count() == 0
    assert alias.acquire(False) and lock.acquire(False) is False
    alias.release()
    assert lock.acquire(False) is True
    lock.release()


def contended():
    lock = threading._PyRLock()
    ready, start, entered, proceed, done = (threading.Lock() for _ in range(5))
    for gate in (ready, start, entered, proceed, done):
        gate.acquire()
    ident = _thread.start_new_thread(
        hold_block, (lock._block, ready, start, entered, proceed, done)
    )
    try:
        # The worker already holds a reference to the backing lock when it is reset.
        try:
            assert ready.acquire(timeout=5)
            lock._at_fork_reinit()
        finally:
            start.release()
        assert entered.acquire(timeout=5)
        assert lock.locked() and not lock._is_owned()
        assert lock.acquire(False) is False
        assert lock.acquire(timeout=0.01) is False
        # A saved recursive state records the owning thread, independently of the caller.
        owned = threading._PyRLock()
        owned._acquire_restore((2, ident))
        assert owned.locked() and not owned._is_owned() and owned._recursion_count() == 0
        try:
            owned.release()
        except RuntimeError as exc:
            assert str(exc) == "cannot release un-acquired lock"
        else:
            raise AssertionError("non-owner released the lock")
        assert owned._release_save() == (2, ident)
    finally:
        proceed.release()
        assert done.acquire(timeout=5)
        deadline = monotonic() + 5
        while _thread._count() and monotonic() < deadline:
            pass
        assert _thread._count() == 0
    assert lock.acquire(False) is True
    lock.release()


factory()
recursive()
contended()
