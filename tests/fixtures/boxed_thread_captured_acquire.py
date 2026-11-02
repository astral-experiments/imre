import _thread
import gc
import sys
import threading
import time
import weakref


def make_worker(lock):
    def acquire():
        lock.acquire()
    return acquire


def exercise(make, joinable):
    old = _thread.allocate_lock() if joinable else _thread.RLock()
    replacement = _thread.allocate_lock()
    old.acquire()
    worker = make(old)
    cell = worker.__closure__[0]
    weak_worker = weakref.ref(worker)
    weak_lock = weakref.ref(old)
    before = _thread._count()
    # CPython may return from start before the callable loads its receiver.
    # Where exposed, the extra reference witnesses the blocked acquire.
    refcount = getattr(sys, "getrefcount", None)
    refs = refcount(old) if refcount else 0
    if joinable:
        thread = threading.Thread(target=worker)
        thread.start()
        ident = thread.ident
        thread.join(0)
        assert thread.is_alive()
    else:
        ident = _thread.start_new_thread(worker, ())
    assert ident != _thread.get_ident()
    deadline = time.monotonic() + 5
    if refcount:
        while refcount(old) <= refs:
            assert time.monotonic() < deadline
            time.sleep(0.001)
    cell.cell_contents = replacement
    del old, worker
    gc.collect()
    assert weak_worker() is not None
    old = weak_lock()
    assert old is not None
    old.release()
    if joinable:
        thread.join(5)
        assert not thread.is_alive()
    else:
        while _thread._count() != before:
            assert time.monotonic() < deadline
            time.sleep(0.001)
        assert not old._is_owned()
    assert old.locked() and not replacement.locked()
    assert cell.cell_contents is replacement
    gc.collect()
    assert weak_worker() is None


exec("def make_source_worker(lock):\n    def acquire():\n        lock.acquire()\n    return acquire\n", globals())
for factory in (make_worker, globals()["make_source_worker"]):
    exercise(factory, False)
    exercise(factory, True)
print("captured lock acquired by its native worker")
