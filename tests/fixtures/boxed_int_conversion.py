import gc
import warnings


def current():
    large = 1 << 140

    class Plain(int):
        pass

    class Override(int):
        def __int__(self):
            return large

    class Indexed:
        def __index__(self):
            return large

    class Converted:
        def __init__(self, result):
            self.result = result

        def __int__(self):
            return self.result

        def __index__(self):
            raise AssertionError("__int__ must take precedence")

    assert int(large) is large
    for text in ("-5", "0", "1", "256"):
        assert int(text) is int(text) + 0
    for text in ("-6", "257"):
        assert int(text) is not int(text)
    assert int("0") is not False and int("1") is not True
    assert type(int(Plain(3))) is int
    assert Plain(3) is not Plain(3) and int(Plain(3)) is int("3")

    saved_id = id(int("173"))
    gc.collect()
    restored = int("173")
    assert id(restored) == saved_id and str(restored) == "173"
    assert int(Override(3)) is large
    assert int(Indexed()) is large
    assert list(map(int, [Converted(large)]))[0] is large

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always")
        result = int(Converted(Plain(7)))
    assert type(result) is int and result == 7
    assert [item.category for item in seen] == [DeprecationWarning]
    assert "__int__ returned non-int (type Plain)" in str(seen[0].message)

    try:
        int(Converted(1.5))
    except TypeError as error:
        assert str(error) == "__int__ returned non-int (type float)"
    else:
        raise AssertionError("non-integer result accepted")

    class Raising(Converted):
        def __int__(self):
            raise LookupError("conversion")

    try:
        int(Raising(None))
    except LookupError as error:
        assert error.args == ("conversion",)
    else:
        raise AssertionError("exception from __int__ was lost")
