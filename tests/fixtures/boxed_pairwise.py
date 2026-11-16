def current():
    from itertools import pairwise

    pulls = []

    class Input:
        def __iter__(self):
            pulls.append("iter")
            return self

        def __next__(self):
            pulls.append("next")
            if len(pulls) == 4:
                raise ValueError("stopped")
            return len(pulls)

    class Tagged(pairwise):
        def __init__(self, iterable, *, tag):
            self.tag = tag

        @property
        def _source(self):
            raise AssertionError("subclass intercepted the iterator")

    cursor = Tagged(Input(), tag=42)
    assert cursor.tag == 42 and pulls == ["iter"]
    assert iter(cursor) is cursor and next(cursor) == (2, 3)
    try:
        next(cursor)
    except ValueError as error:
        assert str(error) == "stopped"
    else:
        raise AssertionError("source exception was lost")
    assert next(cursor, "end") == "end" and pulls.count("next") == 3

    pulls.clear()
    exact = pairwise(Input())
    assert pulls == ["iter"] and next(exact) == (2, 3)

    class Reenter:
        def __init__(self):
            self.items = iter(range(6))

        def __iter__(self):
            return self

        def __next__(self):
            value = next(self.items)
            if value == 0:
                assert next(nested) == (1, 2)
            return value

    nested = pairwise(Reenter())
    assert next(nested) == (0, 3) and list(nested) == [(3, 4), (4, 5)]

    class Override(pairwise):
        def __next__(self):
            return "override", pairwise.__next__(self)

    override = Override([None, 1, 2])
    assert next(override) == ("override", (None, 1))
    Override.__next__ = lambda self: pairwise.__next__(self)
    assert next(override) == (1, 2)

    class StepError(StopIteration):
        pass

    class Interrupt:
        def __init__(self, initial):
            self.initial = initial

        def __iter__(self):
            return self

        def __next__(self):
            if self.initial:
                self.initial = False
                return None
            raise StepError("pause")

    for initial in (False, True):
        stopped = pairwise(Interrupt(initial))
        try:
            next(stopped)
        except StepError as error:
            assert error.args == ("pause",)
        else:
            raise AssertionError("source's StopIteration payload was lost")
        assert next(stopped, "end") == "end"

    import gc
    import weakref

    class Item:
        pass

    items = pairwise(Item() for _ in range(3))
    left, right = next(items)
    previous = weakref.ref(right)
    del left, right
    gc.collect()
    assert previous() is not None and next(items)[0] is previous()
    del items
    gc.collect()
    assert previous() is None

    from importlib import import_module
    import sys
    old_module = sys.modules.pop("itertools")
    try:
        fresh = import_module("itertools").pairwise
        old_cursor = pairwise([None, 1, 2])
        new_cursor = fresh([3, 4])
        saved = next(old_cursor)
        assert saved == (None, 1) and next(new_cursor) == (3, 4)
        assert next(old_cursor) == (1, 2) and saved == (None, 1)
        for cls, other in ((pairwise, new_cursor), (fresh, old_cursor)):
            try:
                cls.__next__(other)
            except TypeError:
                pass
            else:
                raise AssertionError("accepted a cursor from a different defining type")
    finally:
        sys.modules["itertools"] = old_module
