from _thread import allocate_lock, start_new_thread
import logging
import threading


def acquire_on_worker(lock, again, started, finished):
    if not lock.acquire(False):
        started.release()
    lock.acquire()
    if again.acquire(False):
        again.release()
        lock.release()
        finished.release()


def current():
    handler = logging.Handler()
    assert type(logging._lock) is type(threading.RLock())
    assert type(handler.lock) is type(logging._lock) and handler.lock is not logging._lock
    with logging._lock:
        assert logging._lock.acquire(False)
        assert logging._lock._recursion_count() == 2
        logging._lock.release()
        handler.name = "locked-handler"
        assert logging.getHandlerByName(handler.name) is handler
    handler.acquire()
    handler._at_fork_reinit()
    assert not handler.lock._is_owned() and handler.lock.acquire(timeout=0)
    handler.release()

    started = allocate_lock()
    finished = allocate_lock()
    started.acquire()
    finished.acquire()
    with handler.lock:
        start_new_thread(acquire_on_worker, (handler.lock, handler.lock, started, finished))
        assert started.acquire(timeout=5) and handler.lock._is_owned()
    assert finished.acquire(timeout=5) and not handler.lock.locked()

    lock = handler.lock

    class ReplacingLock:
        def __init__(self, truth):
            self.truth = truth

        def __bool__(self):
            handler.lock = lock
            return self.truth

    for truth in (False, True):
        handler.lock = ReplacingLock(truth)
        handler.acquire()
        assert lock._is_owned() is truth
        handler.lock = ReplacingLock(truth)
        handler.release()
        assert not lock._is_owned()

    handler.close()
    assert logging.getHandlerByName("locked-handler") is None
