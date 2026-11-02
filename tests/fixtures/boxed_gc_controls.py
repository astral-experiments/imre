import gc
from weakref import ref


class Node:
    pass


def cycle(events):
    node = Node()
    node.link = node
    return ref(node, events.append)


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
        pressure = [[] for _ in range(12000)]
        assert delayed() is not None and len(pressure) == 12000
        assert gc.collect() >= 1 and events == [immediate, delayed]
        assert delayed() is None and not gc.isenabled()
        pending = cycle(events)
        assert gc.enable() is None and gc.enable() is None and gc.isenabled() is True
        more = [[] for _ in range(32000)]
        assert pending() is None and events == [immediate, delayed, pending]
        assert len(more) == 32000
    finally:
        if was_enabled:
            gc.enable()
        else:
            gc.disable()
