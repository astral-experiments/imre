import os
from os import terminal_size
from posix import terminal_size as posix_size


class BadHint:
    def __iter__(self):
        return self

    def __length_hint__(self):
        raise ValueError("bad hint")

    def __next__(self):
        raise AssertionError("read past the invalid length hint")


class IterTypeError(TypeError):
    pass


class NotIterable:
    def __iter__(self):
        raise IterTypeError("iter failed")


def current():
    assert os.terminal_size is terminal_size is posix_size
    assert (terminal_size.__module__, terminal_size.__name__) == ("os", "terminal_size")
    assert issubclass(terminal_size, tuple)
    size = terminal_size(sequence=(n for n in (83, 29)), dict={})
    assert type(size) is size.__class__ is terminal_size
    assert isinstance(size, tuple) and isinstance(size, terminal_size)
    assert size == (83, 29) and size.columns == size[0] and size.lines == size[-1]
    assert size[:1] == (83,) and hash(size) == hash(tuple(size))
    assert repr(size) == "os.terminal_size(columns=83, lines=29)"
    assert size.__match_args__ == ("columns", "lines")
    assert (size.n_fields, size.n_sequence_fields, size.n_unnamed_fields) == (2, 2, 0)
    try:
        terminal_size(BadHint())
    except ValueError as error:
        assert str(error) == "bad hint"
    else:
        raise AssertionError("ignored iterator length hint")
    try:
        terminal_size(NotIterable())
    except TypeError as error:
        assert type(error) is TypeError and str(error) == "constructor requires a sequence"
    else:
        raise AssertionError("accepted a non-iterable")
    item = []
    assert terminal_size([item, None]).columns is item
    descriptor = terminal_size.columns
    assert descriptor.__get__(size) == 83
    for update in (lambda: setattr(size, "columns", 0), lambda: delattr(size, "lines")):
        try:
            update()
        except AttributeError as error:
            assert str(error) == "readonly attribute"
        else:
            raise AssertionError("modified a read-only field")
    for args, message in (
        ((1,), "constructor requires a sequence"),
        (([1], None), "os.terminal_size() takes a dict as second arg, if any"),
        (([1],), "os.terminal_size() takes a 2-sequence (1-sequence given)"),
        (("ab", {"lines": 1}), "os.terminal_size() got duplicate or unexpected field name(s)"),
    ):
        try:
            terminal_size(*args)
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError(args)
