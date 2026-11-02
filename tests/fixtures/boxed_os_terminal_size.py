import os
import posix


def current():
    owner = os.terminal_size
    assert owner is posix.terminal_size and issubclass(owner, tuple)
    size = owner(sequence=iter([100, 42]), dict={})
    assert type(size) is size.__class__ is owner
    assert isinstance(size, tuple) and isinstance(size, owner)
    assert size == (100, 42) and tuple(size) == (size.columns, size.lines)
    assert hash(size) == hash((100, 42))
    assert repr(size) == "os.terminal_size(columns=100, lines=42)"
    assert owner.__match_args__ == ("columns", "lines")
    assert (size.n_fields, size.n_sequence_fields, size.n_unnamed_fields) == (2, 2, 0)
    assert owner.columns.__get__(size) == 100
    try:
        size.columns = 80
    except AttributeError:
        pass
    else:
        raise AssertionError("mutable terminal_size")
    for args in ((1,), ((1,), None), ((),), ((1, 2, 3),), ((1, 2), {"columns": 3}), ((1, 2), None)):
        try:
            owner(*args)
        except TypeError as error:
            print(error)
        else:
            raise AssertionError(args)

    class BadSequence(TypeError):
        pass

    class BadIterator:
        def __init__(self, error):
            self.error = error

        def __iter__(self):
            raise self.error

    try:
        owner(BadIterator(BadSequence("iter failed")))
    except TypeError as error:
        assert type(error) is TypeError and str(error) == "constructor requires a sequence"
    else:
        raise AssertionError("accepted non-iterable")
    sentinel = ValueError("iter failed")
    try:
        owner(BadIterator(sentinel))
    except ValueError as error:
        assert error is sentinel
    else:
        raise AssertionError("lost iterator error")
