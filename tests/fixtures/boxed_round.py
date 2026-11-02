class Recorder:
    def __round__(self, *args):
        self.args = args
        return self


class Index:
    def __index__(self):
        return -1


def current():
    class FancyInt(int):
        def __int__(self):
            raise AssertionError("int descriptor invoked the override")

    class FancyFloat(float):
        def __round__(self, *args):
            raise AssertionError("float descriptor invoked the override")

    big = 10**42
    assert int.__int__(big) is big and int.__abs__(big) is big
    assert int.__float__(FancyInt(17)) == 17.0
    assert type(int.__int__(True)) is int
    assert float.__int__(FancyFloat(-1.5)) == -1
    assert float.__round__(FancyFloat(25), Index()) == 20.0
    assert bool.__round__ is int.__round__

    f = round
    obj = Recorder()
    obj.__round__ = lambda: 99
    assert f(obj) is obj and obj.args == ()
    assert f(obj, None) is obj and obj.args == ()
    token = []
    assert f(obj, token) is obj and obj.args[0] is token

    for n in (2.5, 3.5, -2.5, -3.5, -0.0, 1e80, True):
        result = f(n)
        print(type(result).__name__, result)
    for n in (25, 35, -25, -35, (1 << 140) + 17, True):
        result = f(n, Index())
        print(type(result).__name__, result)
    big = 1 << 120
    assert f(number=big, ndigits=2) is big
    for n, digits in ((2.675, 2), (-0.25, 0), (-4.9, -1), (25.01, -1),
                      (-25.01, -1), (25.0, -1), (1.79e308, 1000),
                      (float("nan"), 3), (float("-inf"), -2)):
        result = f(n, digits)
        print(type(result).__name__, repr(result))

    for invoke in (lambda: f(), lambda: f(1, 2, 3), lambda: f("1"), lambda: f(list), lambda: f(abs),
                   lambda: f(2.5, 1.5), lambda: f(float("nan")),
                   lambda: f(float("inf")), lambda: f(1.79e308, -308)):
        try:
            invoke()
        except (TypeError, ValueError, OverflowError) as exc:
            print(type(exc).__name__, str(exc))
        else:
            raise AssertionError("expected an error")

