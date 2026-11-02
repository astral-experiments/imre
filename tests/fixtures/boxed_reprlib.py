import reprlib
from reprlib import Repr, recursive_repr


def current():
    formatter = Repr(maxlist=2, maxdict=1, fillvalue="more")
    assert formatter.repr([1, 2, 3]) == "[1, 2, more]"
    assert formatter.repr({"b": 2, "a": 1}) == "{'a': 1, more}"
    assert reprlib.repr((3,)) == "(3,)"
    assert reprlib.aRepr.repr(list(range(8))) == "[0, 1, 2, 3, 4, 5, ...]"

    class Node:
        @recursive_repr("<loop>")
        def __repr__(self):
            return "node:" + repr(self)

    assert repr(Node()) == "node:<loop>"

    class Flaky:
        def __init__(self):
            self.first = True

        @recursive_repr()
        def __repr__(self):
            if self.first:
                self.first = False
                raise ValueError("first call")
            return "recovered"
    item = Flaky()
    try:
        repr(item)
    except ValueError as error:
        assert str(error) == "first call"
    else:
        raise AssertionError("expected custom exception")
    assert repr(item) == "recovered"
    print("container limits, singleton tuple, recursion and cleanup")
