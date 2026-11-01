import functools


def current():
    exported = functools.__dict__["reduce"]
    from functools import reduce

    assert exported is reduce
    marker = object()
    assert reduce(None, [marker]) is marker
    assert reduce(None, [], marker) is marker
    assert reduce(None, [], initial=None) is None

    def items():
        yield 10
        yield 3
        yield 2

    assert reduce(lambda a, b: a - b, items()) == 5
    failure = StopIteration("callback")

    def fail(a, b):
        raise failure

    remaining = items()
    try:
        reduce(fail, remaining)
    except StopIteration as caught:
        assert caught is failure
    else:
        raise AssertionError("callback exception lost")
    assert next(remaining) == 2

    try:
        reduce(None, ())
    except TypeError as error:
        assert str(error) == "reduce() of empty iterable with no initial value"
    else:
        raise AssertionError("empty input accepted")
