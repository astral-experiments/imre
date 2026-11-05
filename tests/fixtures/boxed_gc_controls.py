import gc
from _thread import allocate_lock, start_new_thread
from gc import get_threshold, is_tracked, set_threshold
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


def check_tracking():
    assert not is_tracked(7) and gc.is_tracked([])
    atomic = tuple(range(2))
    container = ([],)
    assert is_tracked(atomic) and is_tracked(container)
    gc.collect()
    assert not is_tracked(atomic) and is_tracked(container)

    for args, kwargs in [((), {}), ((1, 2), {}), ((), {"obj": 1})]:
        try:
            is_tracked(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError((args, kwargs))


def check_thresholds():
    saved = get_threshold()
    was_enabled = gc.isenabled()

    class Indexed:
        def __index__(self):
            set_threshold(11, 22, 33)
            return -7

    try:
        assert set_threshold(0, 2, 3) is None
        assert gc.get_threshold() == (0, 2, 3) and gc.isenabled() == was_enabled
        gc.set_threshold(Indexed(), True)
        assert get_threshold() == (-7, 1, 33)
        set_threshold(2**31 - 1)
        assert get_threshold() == (2**31 - 1, 1, 33)
        set_threshold(0, -(2**31))
        before = (0, -(2**31), 33)
        assert get_threshold() == before
        for fn, args, kwargs, exc, message in [
            (get_threshold, (1,), {}, TypeError, "gc.get_threshold() takes no arguments (1 given)"),
            (get_threshold, (), {"x": 1}, TypeError, "gc.get_threshold() takes no keyword arguments"),
            (set_threshold, (), {}, TypeError, "gc.set_threshold requires 1 to 3 arguments"),
            (set_threshold, (1, 2, 3, 4), {}, TypeError, "gc.set_threshold requires 1 to 3 arguments"),
            (set_threshold, (), {"threshold0": 1}, TypeError, "set_threshold() takes no keyword arguments"),
            (set_threshold, (1, 2, 3.0), {}, TypeError, "'float' object cannot be interpreted as an integer"),
            (set_threshold, (1, 2**31), {}, OverflowError, "signed integer is greater than maximum"),
            (set_threshold, (-(2**31) - 1,), {}, OverflowError, "signed integer is less than minimum"),
            (set_threshold, (2**100,), {}, OverflowError, "Python int too large to convert to C long"),
        ]:
            try:
                fn(*args, **kwargs)
            except exc as error:
                assert str(error) == message
            else:
                raise AssertionError((fn, args, kwargs))
            assert get_threshold() == before
    finally:
        set_threshold(*saved)


def check_header_pressure():
    from datetime import timedelta
    from hashlib import blake2s, sha256

    class Delta(timedelta):
        pass

    target = Node()

    def ignore(wr):
        pass

    def new_ref():
        return ref(target, ignore)

    saved = get_threshold()
    enabled = gc.isenabled()
    try:
        for factory, keep, collected in (
            (timedelta, True, False),
            (sha256, True, False),
            (Delta, True, True),
            (blake2s, True, True),
            (new_ref, True, True),
            (new_ref, False, False),
        ):
            gc.disable()
            retained = [None] * 128
            callbacks = []
            gc.collect()
            victim = cycle(callbacks)
            set_threshold(32, 10000, 10000)
            gc.enable()
            for i in range(len(retained)):
                if keep:
                    retained[i] = factory()
                else:
                    factory()
            gc.disable()
            assert (victim() is None) == collected, (factory, keep)
            assert callbacks == ([victim] if collected else [])
    finally:
        set_threshold(*saved)
        if enabled:
            gc.enable()
        else:
            gc.disable()


def current():
    check_thresholds()
    check_header_pressure()
    was_enabled = gc.isenabled()
    events = []
    try:
        assert gc.disable() is None and gc.disable() is None
        assert gc.isenabled() is False
        check_tracking()
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
