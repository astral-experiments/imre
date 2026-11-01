"""A suspended generator retains its context manager and unwinds it once."""
events = []


class Scope:
    def __init__(self, suppress):
        self.suppress = suppress

    def __enter__(self):
        events.append("enter")
        return 7

    def __exit__(self, kind, value, traceback):
        if kind is None:
            assert value is None and traceback is None
            events.append("exit")
        else:
            assert kind is type(value) and traceback is not None
            events.append(kind.__name__)
        return self.suppress


def rows(suppress):
    with Scope(suppress) as value:
        yield value
    yield "after"


def drained(iterator):
    try:
        next(iterator)
    except StopIteration:
        return
    raise AssertionError("generator should have finished")


def current():
    events.clear()
    iterator = rows(False)
    assert events == []
    assert next(iterator) == 7 and events == ["enter"]
    assert next(iterator) == "after" and events == ["enter", "exit"]
    drained(iterator)

    iterator = rows(True)
    assert next(iterator) == 7
    assert iterator.throw(KeyError("injected")) == "after"
    drained(iterator)
    assert events == ["enter", "exit", "enter", "KeyError"]

    iterator = rows(False)
    assert next(iterator) == 7
    iterator.close()
    iterator.close()
    assert events[-2:] == ["enter", "GeneratorExit"]
    print("boxed generator with ok")
