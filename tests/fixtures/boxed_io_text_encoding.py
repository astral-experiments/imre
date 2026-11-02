import _io
from io import TextIOWrapper, text_encoding


def current():
    assert text_encoding is _io.text_encoding
    assert TextIOWrapper is _io.TextIOWrapper
    supplied = object()
    calls = []

    class Level:
        def __index__(self):
            calls.append(1)
            return 0

    assert text_encoding(supplied, Level()) is supplied
    assert calls == [1]
    assert text_encoding.__self__ is _io
    assert text_encoding.__module__ == "_io"
    assert text_encoding(None) is text_encoding(None)
    assert text_encoding(None) in ("locale", "utf-8")
    for invalid, error in ((2.5, TypeError), (2**40, OverflowError)):
        try:
            text_encoding(supplied, invalid)
        except error:
            pass
        else:
            raise AssertionError("invalid stacklevel was accepted")
    return "ok"
