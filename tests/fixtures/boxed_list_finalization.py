events = []
failure = ValueError("stop")


class Token:
    def __init__(self, value):
        self.value = value

    def __del__(self):
        events.append(self.value)


def item(value):
    if value == 3:
        raise failure
    return Token(value)


def completed():
    values = [item(i) for i in (1, 2)]
    alias = values
    del values
    assert events == []
    del alias
    assert events == [2, 1]


def partial():
    keep = item(0)
    try:
        [item(i) if i else keep for i in (0, 1, 2, 3)]
    except ValueError as exc:
        assert exc is failure
        assert events == [2, 1]
        assert keep.value == 0
    else:
        raise AssertionError("missing exception")
    del keep
    assert events == [2, 1, 0]


def interrupted_call():
    def unreachable(*values):
        raise AssertionError("failed argument reached the callee")

    try:
        unreachable([item(1), [item(2), item(4)]], item(3))
    except ValueError as exc:
        assert exc is failure
    assert events == [4, 2, 1]


def interrupted_method():
    class Sink:
        def consume(self, *values):
            raise AssertionError("failed argument reached the method")

    sink = Sink()
    try:
        sink.consume([item(1), [item(2), item(4)]], item(3))
    except ValueError as exc:
        assert exc is failure
        assert events == [4, 2, 1]


for run in (completed, partial, interrupted_call, interrupted_method):
    events.clear()
    run()
