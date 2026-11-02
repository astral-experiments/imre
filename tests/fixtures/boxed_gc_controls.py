import gc
from _thread import allocate_lock, start_new_thread
from weakref import ref


class Node:
    pass


def cycle(events):
    node = Node()
    node.link = node
    return ref(node, events.append)


def enable_on_worker(isenabled, enable, done):
    if not isenabled():
        enable()
        done.release()


def current():
    was_enabled = gc.isenabled()
    events = []
    try:
        assert gc.disable() is None and gc.disable() is None
        assert gc.isenabled() is False
        delayed = cycle(events)
        node = Node()
        immediate = ref(node, events.append)
        del node
        assert events == [immediate] and immediate() is None
        pressure = [[] for _ in range(4000)]
        assert delayed() is not None and len(pressure) == 4000
        assert gc.collect() >= 1 and events == [immediate, delayed]
        assert delayed() is None and not gc.isenabled()
        pending = cycle(events)
        assert gc.enable() is None and gc.enable() is None and gc.isenabled() is True
        more = [[] for _ in range(8000)]
        assert pending() is None and events == [immediate, delayed, pending]
        assert len(more) == 8000

        done = allocate_lock()
        done.acquire()
        gc.disable()
        start_new_thread(enable_on_worker, (gc.isenabled, gc.enable, done))
        assert done.acquire(timeout=5) and gc.isenabled()
    finally:
        if was_enabled:
            gc.enable()
        else:
            gc.disable()
