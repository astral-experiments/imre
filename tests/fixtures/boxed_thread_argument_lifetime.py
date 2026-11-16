"""A blocked worker keeps its bound arguments, independently of their input container."""
import _thread
import gc
import threading
import time
import weakref


def wait(ready, gate):
    ready.acquire()
    gate.acquire()


def acquire_keyword(ready, /, *, gate=None):
    ready.acquire()
    gate.acquire(False)


def exercise(keywords, keep_container):
    ready = threading.Lock()
    gate = threading.Lock()
    gate.acquire()
    caller = _thread.get_native_id()
    callbacks = []

    def notified(ref):
        callbacks.append((ref, ref() is None, _thread.get_native_id()))

    gate_ref = weakref.ref(gate, notified)
    if keywords:
        source = {"ready": ready, "gate": gate}
        thread = threading.Thread(target=wait, kwargs=source, daemon=True)
    else:
        source = [ready, gate]
        thread = threading.Thread(target=wait, args=source, daemon=True)
    thread.start()
    try:
        deadline = time.monotonic() + 5
        while not ready.locked():
            assert time.monotonic() < deadline
            time.sleep(0.001)
        assert isinstance(thread.native_id, int) and thread.native_id != caller
        assert thread.is_alive()

        # Only the already-bound gate can be seen by the worker. This lock is added later.
        late = threading.Lock()
        late_ref = weakref.ref(late, notified)
        source.clear()
        if keywords:
            source["gate"] = late
            thread._kwargs = {}
        else:
            source.append(late)
            thread._args = ()
        held = [source] if keep_container else []
        del source, late, gate
        gc.collect()
        assert gate_ref() is not None
        if keep_container:
            assert late_ref() is not None and not callbacks
            held.clear()
            gc.collect()
        assert late_ref() is None
        assert len(callbacks) == 1 and callbacks[0][0] is late_ref
        assert callbacks[0][1:] == (True, caller)
    finally:
        # Retain the actual bound gate until join, making its final release unambiguous.
        last_owner = gate_ref()
        if last_owner is not None:
            last_owner.release()
        thread.join(5)
    assert not thread.is_alive() and last_owner.locked()
    del last_owner
    gc.collect()
    assert gate_ref() is None and len(callbacks) == 2
    assert callbacks[1][0] is gate_ref and callbacks[1][1:] == (True, caller)


for keywords in (False, True):
    for keep_container in (False, True):
        exercise(keywords, keep_container)


for supplied in (False, True):
    ready, default, explicit = (threading.Lock() for _ in range(3))
    acquire_keyword.__kwdefaults__["gate"] = default
    worker = threading.Thread(
        target=acquire_keyword, args=(ready,), kwargs={"gate": explicit} if supplied else {}
    )
    worker.start()
    worker.join(5)
    assert not worker.is_alive() and ready.locked()
    assert explicit.locked() == supplied and default.locked() != supplied
