import functools


def current():
    import _functools
    import sys
    import warnings

    exported = functools.__dict__["reduce"]
    from functools import reduce

    assert exported is reduce is _functools.reduce
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

    # Without the accelerator, Python 3.14 still accepts the old keyword names.
    sys.modules.pop("functools")
    sys.modules["_functools"] = None
    try:
        import functools as fallback
    finally:
        sys.modules["functools"] = functools
        sys.modules["_functools"] = _functools
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert fallback.reduce(function=lambda x, y: x - y, sequence=items(), initial=20) == 5
        try:
            fallback.reduce(None, items(), function=None)
        except TypeError as error:
            assert "multiple values for argument 'function'" in str(error)
        else:
            raise AssertionError("duplicate function accepted")
    assert [w.category for w in caught] == [DeprecationWarning, DeprecationWarning]

    import gc
    import weakref

    class Seed:
        pass

    def retained_on_next_pull(reducer):
        observed = []

        def stream():
            seed = Seed()
            ref = weakref.ref(seed)
            yield seed
            del seed
            yield 1
            gc.collect()
            observed.append(ref() is not None)
            yield 2

        assert reducer(lambda previous, item: None, stream()) is None
        return observed

    assert retained_on_next_pull(reduce) == [True]
    assert retained_on_next_pull(fallback.reduce) == [False]
