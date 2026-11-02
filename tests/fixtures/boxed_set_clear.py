import gc
import weakref


def current():
    class Key:
        pass

    key = Key()
    ref = weakref.ref(key)
    values = {key}
    clear = values.clear
    del key
    gc.collect()
    assert ref() is not None
    assert clear() is None and not values
    gc.collect()
    assert ref() is None
    assert clear() is None

    class Subset(set):
        def clear(self):
            return "override"

    sub = Subset([1, 2])
    assert sub.clear() == "override"
    assert sub.__contains__(1) and sub.__contains__(2)
    assert set.clear(sub) is None
    assert not sub.__contains__(1) and not sub.__contains__(2)
    assert not hasattr(frozenset(), "clear")

    values.add(3)
    for args, kwargs, message in (
        ((), {}, "unbound method set.clear() needs an argument"),
        ((frozenset(),), {}, "descriptor 'clear' for 'set' objects doesn't apply to a 'frozenset' object"),
        ((values, 1), {}, "set.clear() takes no arguments (1 given)"),
        ((values,), {"item": 1}, "set.clear() takes no keyword arguments"),
    ):
        try:
            set.clear(*args, **kwargs)
        except TypeError as exc:
            assert str(exc) == message
        else:
            raise AssertionError("invalid clear succeeded")
    assert values == {3}


if __name__ == "__main__":
    current()
