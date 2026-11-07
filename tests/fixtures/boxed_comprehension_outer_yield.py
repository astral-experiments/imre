"""Only the creating frame suspends while acquiring the first iterable."""


def display():
    x = "outer"
    return [x * 2 for x in (yield "list")], x


def delegated_values():
    yield "genexpr"
    return [2, 3]


def genexpr():
    return (x * 2 for x in [y for y in (yield from delegated_values())])


def current():
    parent = display()
    assert next(parent) == "list"
    try:
        parent.send([7, 8])
    except StopIteration as done:
        assert done.value == ([14, 16], "outer")
    else:
        raise AssertionError("outer generator should return its list")

    parent = genexpr()
    assert next(parent) == "genexpr"
    try:
        next(parent)
    except StopIteration as done:
        assert list(done.value) == [4, 6]
    else:
        raise AssertionError("outer generator should return its genexpr")
