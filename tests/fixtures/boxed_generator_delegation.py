events = []


def rows(groups):
    events.append("start")
    for label, values in groups:
        for value in values:
            yield label, value
    else:
        events.append("end")


def wrapped(groups):
    yield from rows(groups)
    return 42


def capture(value):
    def inner():
        yield value
        yield from (value, value)

    return inner


def unpack():
    yield "before"
    for left, right in [(1,)]:
        yield left, right


def fail(cause):
    raise cause


def stopped(cause):
    yield fail(cause)


def raise_after_yield(error, cause):
    yield 1
    raise error from cause


def annotated():
    item: missing_annotation = 7
    yield item
    pending: missing_annotation
    yield pending


def current():
    two = wrapped([("a", [1, 2]), ("b", [3])])
    print(two.gi_code is wrapped.__code__, events)
    for first in two:
        print(first, events)
        break
    print(next(two), next(two), events)
    try:
        next(two)
    except StopIteration as error:
        print(error.args, next(two, "closed"), events)

    a, b = capture(1), capture(2)
    print(a.__code__ is b.__code__, list(a()), list(b()))
    bad = unpack()
    print(next(bad))
    try:
        next(bad)
    except ValueError as error:
        print(str(error), next(bad, "closed"))

    cause = StopIteration("explicit")
    g = stopped(cause)
    try:
        next(g)
    except RuntimeError as error:
        print(str(error), error.__cause__ is cause, error.__context__ is cause)
        print(error.__suppress_context__, next(g, "closed"))

    for error, explicit in ((ValueError("bad row"), cause), (cause, None)):
        g = raise_after_yield(error, explicit)
        print(next(g))
        try:
            next(g)
        except (ValueError, RuntimeError) as caught:
            print(type(caught).__name__, caught.__cause__ is cause, next(g, "closed"))

    paused = wrapped([("gc", [1, 2])])
    print(next(paused))
    # Collect temporary objects while both generators are suspended.
    for n in range(5000):
        temp = [str(n)]
    print(list(paused))


    stream = ((key, [key * 2]) for key in [3, 4])
    print(list(wrapped(stream)))
    expression = (fail(cause) for item in [None])
    try:
        next(expression)
    except RuntimeError as error:
        print(error.__cause__ is cause, error.__context__ is cause)

    values = annotated()
    print(next(values))
    try:
        next(values)
    except UnboundLocalError as error:
        print(str(error), next(values, "closed"))
