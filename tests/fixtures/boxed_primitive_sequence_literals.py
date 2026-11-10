import gc


def literals():
    return [None, False, 0, True, 1, 255, 256, ..., 0xff]


def list_operands(first, second, third):
    return [first(), second(), third()]


def tuple_operands(first, second, third):
    return (first(), second(), third())


def nested_operands(first, second, third):
    return [[first(), second()], (third(),)]


def constants():
    return ["two words 🐍"], ("two words 🐍", -0.0, (None, -1))


def current():
    check_set_operands()
    left, right = literals(), literals()
    left[0] = 7
    gc.collect()
    assert left is not right and right[0] is None
    values = (None, False, 0, True, 1, 255, 256, ..., 0xff)
    assert all(x is y for x, y in zip(right, values))
    calls = []

    def value(n):
        calls.append(n)
        return n

    assert [1, value(2), *[3, 4], value(5)] == [1, 2, 3, 4, 5]
    assert calls == [2, 5]
    assert [0, 257, -6, True] == list((0, 257, -6, True))
    first, a = constants()
    second, b = constants()
    assert first is not second and a is b
    assert first[0] is second[0] is a[0]

    released = []

    class Token:
        def __init__(self, n):
            self.n = n

        def __del__(self):
            released.append(self.n)

    def fail():
        raise LookupError("operand")

    for build in (list_operands, tuple_operands, nested_operands):
        released.clear()
        try:
            build(lambda: Token(1), lambda: Token(2), fail)
        except LookupError as error:
            assert error.args == ("operand",)
            assert released == [2, 1]
        else:
            raise AssertionError("third operand must raise")
    released.clear()
    held = [Token(3), Token(4)]
    try:
        tuple_operands(lambda: held, lambda: held, fail)
    except LookupError:
        assert [value.n for value in held] == [3, 4] and released == []
    else:
        raise AssertionError("third operand must raise")
    print(right, values)


def check_set_operands():
    events = []

    class Key:
        def __init__(self, name):
            self.name = name
            events.append("make-" + name)

        def __hash__(self):
            events.append("hash-" + self.name)
            if self.name == "bad":
                raise ValueError("stop")
            return 17

    try:
        {Key("bad"), Key("next")}
    except ValueError as exc:
        assert exc.args == ("stop",)
    else:
        raise AssertionError("expected failing hash")
    assert events == ["make-bad", "make-next", "hash-bad"]
    events.clear()

    def stream():
        yield Key("ok")
        yield Key("bad")

    try:
        {*stream(), Key("unreached")}
    except ValueError as exc:
        assert exc.args == ("stop",)
    else:
        raise AssertionError("expected failing hash while unpacking")
    assert events == ["make-ok", "hash-ok", "make-bad", "hash-bad"]
