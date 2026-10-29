events = []


def rows(groups):
    events.append("start")
    for label, values in groups:
        for value in values:
            yield label, value
    else:
        events.append("end")


def wrapped(groups):
    """Delegate the grouped rows."""
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
    yield -1, +1, ~1, not 1
    raise error from cause


def annotated():
    item: missing_annotation = 7
    yield item
    pending: missing_annotation
    yield pending


def visited(trace, label, value):
    trace.append(label)
    return value


def byte_chunks():
    for value in [0, 1]:
        yield b'a\x00' b'\xff', br'\n', b''


def removed(items, pairs):
    for first, second in pairs:
        del items[first], items[second]
        yield list(items)


def checked_members(values):
    for value in values:
        assert value in {0, 1}, value
        yield value


def loop_jumps(rows):
    for row in rows:
        for value in row:
            if value < 0:
                break
            if value == 0:
                continue
            yield value
        else:
            yield "exhausted"
    else:
        yield "done"


def totals(values, cells, trace):
    total = 0
    for value in values:
        total += value
        cells[visited(trace, 'index', 0)] += visited(trace, 'rhs', total)
        yield total, cells[0]


def indexed_values(owner, trace):
    for word in ['first', 'second']:
        visited(trace, 'receiver', owner)[visited(trace, 'key', 0)] = visited(trace, 'rhs', word)
        yield owner[0]


def boolean_values(items, trace):
    for item in items:
        yield (item and visited(trace, 'and', 7)) or visited(trace, 'or', 9)


def three_then_error():
    yield 1
    yield 2
    yield 3
    raise AssertionError("unpacking requested a fourth item")


def unpacked(rows):
    for row in rows:
        left, [middle, right] = row
        yield left, middle, right


class RestartTail:
    def __init__(self):
        self.calls = 0
        self.stream = iter((1, 2, 3))

    def __iter__(self):
        self.calls += 1
        if self.calls == 1:
            return self
        return iter([7, 8])

    def __next__(self):
        return next(self.stream)


def starred(rows):
    for first, *rest in rows:
        *middle, last = rest
        yield first, middle, last


def stored(rows, box, cells):
    for box.value, cells[0] in rows:
        cells[1], box.value = box.value, cells[0]
        yield cells[0], cells[1], box.value


def chained(values, box):
    for value in values:
        local = box.value = [value]
        local.append(value + 1)
        yield local is box.value, box.value


class ClearingTargets:
    def __init__(self, source):
        self.source = source
        self.values = []

    def __setitem__(self, index, value):
        self.values.append(value)
        if index == 0:
            self.source.clear()
            for i in range(9000):
                garbage = [i, -i]


def assigning_cleared_values(source, targets, starred):
    if starred:
        targets[0], *targets[1], targets[2] = source
    else:
        targets[0], targets[1], targets[2] = source
    yield targets.values


def current():
    try:
        first, second = three_then_error()
    except ValueError as error:
        assert str(error) == "too many values to unpack (expected 2)"
    else:
        raise AssertionError("expected too many values")

    class Box:
        pass

    print(list(stored([(1, 2), (3, 4)], Box(), [0, 0])))
    print(list(chained([2, 3], Box())))
    print(list(unpacked([(1, [2, 3]), (4, (5, 6))])))
    print(list(starred([[1, 2], [3, 4, 5, 6]])))
    for target in ('*left, *right', '*left'):
        try:
            compile('def split(row):\n    ' + target + ' = row\n    yield left\n', '<split>', 'exec')
        except SyntaxError:
            print('invalid star')
    pulls = []
    values = (x + y for x in visited(pulls, "outer", [1, 2])
              for y in visited(pulls, x, range(x)) if y == 0)
    assert pulls == ["outer"]
    assert next(values) == 1 and pulls == ["outer", 1]
    print(list(byte_chunks()))
    prefix, *tail, final = RestartTail()
    assert (prefix, tail, final) == (1, [7], 8)
    assert list(starred([RestartTail()])) == [(1, [7], 8)]
    selected = checked_members([1, 2])
    assert next(selected) == 1
    try:
        next(selected)
    except AssertionError as error:
        assert error.args == (2,)
    else:
        raise AssertionError("expected failed membership assertion")
    assert list(loop_jumps([[0, 1], [2, -1, 3], []])) == [1, "exhausted", 2, "exhausted", "done"]
    assert list(values) == [2] and pulls == ["outer", 1, 2]

    trace = []
    print(list(totals([3, -1, 5], [0], trace)), trace)

    items = {1: 10, 2: 20, 3: 30}
    deletions = removed(items, [(2, 3), (1, 4)])
    print(next(deletions))
    try:
        next(deletions)
    except KeyError as error:
        print(error.args, items)

    saved = -1
    assert any((saved := item) for item in [0, 4, 9]) and saved == 4
    assert [[(saved := item) for item in row] for row in [[2, 7]]] == [[2, 7]]
    assert saved == 7

    def empty():
        assert not any((saved := item) for item in [])
        try:
            return saved
        except UnboundLocalError:
            return "unbound"

    assert empty() == "unbound"

    comparisons = []
    class Item:
        def __lt__(self, other):
            return self
        def __bool__(self):
            comparisons.append(1)
            return len(comparisons) > 1
    def choose(item):
        if item < item < item:
            yield "taken"
        else:
            yield "else"
    assert list(choose(Item())) == ["else"]
    assert comparisons == [1]
    owner, trace = ['initial'], []
    print(list(indexed_values(owner, trace)), owner, trace)
    trace = []
    assert list(boolean_values([0, 2], trace)) == [9, 7]
    assert trace == ['or', 'and']
    print(wrapped.__doc__)
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

    try:
        raise LookupError("caller")
    except LookupError as outer:
        try:
            next(stopped(cause))
        except RuntimeError as error:
            assert error.__context__ is error.__cause__ is cause
            assert cause.__context__ is outer

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

    def relay(values):
        yield from values

    nested = [42]
    for _ in range(6000):
        nested = relay(nested)
    print(next(nested), list(nested))

    def reraised():
        yield "ready"
        raise

    stream = reraised()
    print(next(stream))
    try:
        raise LookupError("original")
    except LookupError as original:
        try:
            next(stream)
        except LookupError as caught:
            frames = []
            traceback = caught.__traceback__
            while traceback is not None:
                frames.append(traceback.tb_frame.f_code.co_name)
                traceback = traceback.tb_next
            print(caught is original, frames, next(stream, "closed"))

    stream = reraised()
    next(stream)
    try:
        next(stream)
    except RuntimeError as caught:
        print(caught.args, caught.__traceback__.tb_next.tb_frame.f_code.co_name, next(stream, "closed"))

    def explicit(error):
        yield "ready"
        raise error

    try:
        raise LookupError("explicit")
    except LookupError as original:
        stream = explicit(original)
        next(stream)
        try:
            next(stream)
        except LookupError as caught:
            print(caught is original, caught.__traceback__.tb_next.tb_frame.f_code.co_name, next(stream, "closed"))

    for use_starred in [False, True]:
        source = [[0], [1], [2]]
        targets = ClearingTargets(source)
        print(next(assigning_cleared_values(source, targets, use_starred)))
