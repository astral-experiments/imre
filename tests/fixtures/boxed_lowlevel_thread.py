import _thread
import sys
from time import monotonic


def handoff(count, ready, proceed, finished):
    if count() == 1:
        ready.release()
    proceed.acquire()
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
