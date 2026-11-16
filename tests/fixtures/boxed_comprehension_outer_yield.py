"""Only the creating frame suspends while acquiring the first iterable."""


def display():
    x = "outer"
    return [x * 2 for x in (yield "list")], x


def delegated_values():
    yield "genexpr"
    return [2, 3]


def genexpr():
    return (x * 2 for x in [y for y in (yield from delegated_values())])


def acquire(events):
    try:
        return (yield "rows")
    finally:
        events.append("released")


def delegated(events):
    return (x * 2 for x in ((yield from acquire(events)) or ()))


async def rows():
    yield 2
    yield 3


async def async_display():
    yield [x * 2 async for x in ((yield "rows") or rows()) if x > 2]


def completed(operation):
    try:
        operation.send(None)
    except StopIteration as done:
        return done.value
    raise AssertionError("unexpected pending operation")


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

    events = []
    parent = delegated(events)
    assert next(parent) == "rows"
    try:
        parent.send([2, 4])
    except StopIteration as done:
        assert list(done.value) == [4, 8]
    else:
        raise AssertionError("delegation should return its genexpr")
    assert events == ["released"]

    parent = delegated(events)
    assert next(parent) == "rows"
    parent.close()
    parent = delegated(events)
    assert next(parent) == "rows"
    error = RuntimeError("cancelled")
    try:
        parent.throw(error)
    except RuntimeError as caught:
        assert caught is error
    else:
        raise AssertionError("delegation should propagate the exception")
    assert events == ["released"] * 3

    parent = async_display()
    assert completed(parent.__anext__()) == "rows"
    assert completed(parent.asend(None)) == [6]
    assert completed(parent.aclose()) is None
