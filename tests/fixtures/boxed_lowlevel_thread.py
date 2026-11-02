import _thread
import sys
from time import monotonic


def handoff(count, ready, proceed, finished):
    if count() == 1:
        ready.release()
    proceed.acquire()
    finished.release()


def worker_depth(get, alias, set_depth, ready, proceed, finished):
    if get() == 0 and alias() == 0:
        set_depth(7)
        ready.release()
    proceed.acquire()
    if alias() == 7 and get() == 7:
        finished.release()


def forbid(event, values):
    if event == "_thread.start_new_thread":
        assert values[0] is handoff and values[1] is args and values[2] is None
        raise RuntimeError("thread launch vetoed")


ready, proceed, finished = (_thread.LockType(), _thread.LockType(), _thread.LockType())
for lock in (ready, proceed, finished):
    lock.acquire()
assert _thread._count() == 0
args = (_thread._count, ready, proceed, finished)
ident = _thread.start_new_thread(handoff, args)
assert type(ident) is int and ready.acquire(timeout=5)
assert finished.locked() and _thread._count() == 1
proceed.release()
assert finished.acquire(timeout=5)

get_depth = sys.get_coroutine_origin_tracking_depth
set_depth = sys.set_coroutine_origin_tracking_depth
previous = get_depth()
ready, proceed, finished = (_thread.LockType(), _thread.LockType(), _thread.LockType())
for lock in (ready, proceed, finished):
    lock.acquire()
try:
    set_depth(2)
    _thread.start_new_thread(worker_depth, (get_depth, get_depth, set_depth, ready, proceed, finished))
    assert ready.acquire(timeout=5) and get_depth() == 2
    set_depth(4)
    proceed.release()
    assert finished.acquire(timeout=5) and get_depth() == 4
finally:
    if proceed.locked():
        proceed.release()
    set_depth(previous)

deadline = monotonic() + 5
while _thread._count() and monotonic() < deadline:
    pass
assert _thread._count() == 0

sys.addaudithook(forbid)
try:
    _thread.start_new_thread(handoff, args)
except RuntimeError as exc:
    assert str(exc) == "thread launch vetoed"
else:
    raise AssertionError("audit hook did not prevent launch")
assert _thread._count() == 0
