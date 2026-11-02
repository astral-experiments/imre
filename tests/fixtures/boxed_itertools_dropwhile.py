from itertools import dropwhile


def current():
    calls = []

    class Truth:
        def __bool__(self):
            raise ArithmeticError("truth")

    def predicate(item):
        calls.append(item)
        if item == 1:
            raise ValueError("predicate")
        if item == 2:
            return Truth()
        if item == 3:
            return True
        return False

    class Source:
        def __init__(self):
            self.values = iter([OSError("source"), 1, 2, 3, 4, StopIteration("pause"), 5])

        def __iter__(self):
            calls.append("iter")
            return self

        def __next__(self):
            item = next(self.values)
            if isinstance(item, BaseException):
                raise item
            return item

    class Cursor(dropwhile):
        def __init__(self, *args, label):
            self.label = label

    source = Source()
    try:
        dropwhile.__new__(dict, predicate, source)
    except TypeError:
        pass
    else:
        raise AssertionError("allocated an unrelated type")
    assert not calls
    it = Cursor(predicate, source, label=7)
    assert iter(it) is it and it.label == 7 and calls == ["iter"]
    for error in (OSError, ValueError, ArithmeticError):
        try:
            next(it)
        except error:
            pass
        else:
            raise AssertionError("dropwhile swallowed an error")
    assert next(it) == 4
    assert next(it, None) is None
    assert next(it) == 5 and next(it, None) is None
    assert calls == ["iter", 1, 2, 3, 4]

    def reenter(item):
        if item == 1:
            assert next(it) == 0
        else:
            assert item == 0
        return item

    it = dropwhile(reenter, [1, 0, 3])
    assert list(it) == [3]
