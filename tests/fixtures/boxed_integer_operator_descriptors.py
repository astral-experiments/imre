def current():
    events = []

    class Number(int):
        def __rshift__(self, other):
            events.append("forward")
            return "override"

    class Restored(Number):
        __rrshift__ = int.__rrshift__

    class Sibling(int):
        __rshift__ = int.__rshift__

    class Swapped(Number):
        __rrshift__ = int.__rshift__

    class Foreign:
        def __index__(self):
            raise AssertionError("the int wrapper must decline")

    wide = Number(-(1 << 150) + 7)
    for forward, reflected, sign, expected in (
        ("__lshift__", "__rlshift__", "<<", int(wide) << 2),
        ("__rshift__", "__rrshift__", ">>", int(wide) >> 2),
        ("__floordiv__", "__rfloordiv__", "//", int(wide) // 2),
        ("__mod__", "__rmod__", "%", int(wide) % 2),
    ):
        for name, receiver, other, expression in (
            (forward, wide, 2, "self" + sign + "value"),
            (reflected, 2, wide, "value" + sign + "self"),
        ):
            descriptor = getattr(int, name)
            assert descriptor is int.__dict__[name] is getattr(bool, name)
            assert descriptor.__objclass__ is int
            assert descriptor.__qualname__ == "int." + name
            assert descriptor.__doc__ == "Return " + expression + "."
            assert descriptor.__text_signature__ == "($self, value, /)"
            assert descriptor(receiver, other) == expected
            wrapper = descriptor.__get__(receiver)
            assert wrapper.__self__ is receiver
            assert wrapper(other) == expected
            assert wrapper(Foreign()) is NotImplemented
        if sign in ("<<", ">>"):
            try:
                getattr(int, reflected)(-1, 7)
            except ValueError as e:
                assert str(e) == "negative shift count"
            else:
                raise AssertionError("negative count accepted")
        else:
            try:
                getattr(int, reflected)(0, 7)
            except ZeroDivisionError:
                pass
            else:
                raise AssertionError("zero divisor accepted")
    assert Number(8) >> Restored(2) == "override"
    assert Number(8) >> Sibling(2) == "override"
    assert Number(8) >> Swapped(2) == 0
    assert events == ["forward", "forward"]
    assert int.__rshift__(Number(8), True) == 4
    return True
