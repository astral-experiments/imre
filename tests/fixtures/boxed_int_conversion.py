import gc
import warnings


def current():
    for left in (False, True):
        for right in (False, True):
            assert (left & right) is (left and right)
            assert (left | right) is (left or right)
            assert (left ^ right) is (left != right)
    assert type(True | 0) is int
    assert type(1 & False) is int

    large = 1 << 140
    assert large == int(str(large))
    assert large != large + 1

    class ComparisonOverride(int):
        def __eq__(self, other):
            return "equal"

        def __ne__(self, other):
            return "unequal"

    override = ComparisonOverride(large)
    assert (large == override) == "equal"
    assert (large != override) == "unequal"

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

    class Label(str):
        def __repr__(self):
            # Put the last input character at the diagnostic boundary.
            return "é" * 199 + self[-1] + "suffix"

    for text, base in (("9\ud800", 10), ("é" * 195 + "\ud800'", 16),
                       (Label("bad!"), 16), (Label("9\ud800"), 16)):
        try:
            int(text, base)
        except ValueError as error:
            assert error.args == (f"invalid literal for int() with base {base}: {repr(text)[:200]}",)
        else:
            raise AssertionError("accepted an invalid integer literal")

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
