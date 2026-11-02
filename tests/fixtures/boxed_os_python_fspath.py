import os
import posix
from os import _fspath


def current():
    assert _fspath is os._fspath and _fspath is not os.fspath
    assert os.fspath is posix.fspath

    class Text(str):
        pass

    class Data(bytes):
        pass

    for value in (Text("path"), Data(b"path")):
        assert _fspath(path=value) is value

    events = []

    class OwnerDescriptor:
        def __get__(self, instance, owner):
            events.append((instance, owner))
            return lambda path: path.value

    class Path:
        __fspath__ = OwnerDescriptor()

    path = Path()
    path.value = b"original"
    path.__fspath__ = lambda: b"instance"
    assert _fspath(path) == b"original"
    assert events == [(None, Path)]

    class Raises:
        def __fspath__(self):
            raise problem

    for problem in (AttributeError("from user"), TypeError("from user")):
        try:
            _fspath(Raises())
        except (TypeError, AttributeError) as exc:
            assert exc is problem
        else:
            raise AssertionError("did not propagate the original exception")

    class Disabled:
        __fspath__ = None

    class Bad:
        def __fspath__(self):
            return 42

    for value, expected in (
        (object(), "expected str, bytes or os.PathLike object, not object"),
        (Disabled(), "expected str, bytes or os.PathLike object, not Disabled"),
        (Bad(), "expected Bad.__fspath__() to return str or bytes, not int"),
    ):
        try:
            _fspath(value)
        except TypeError as exc:
            assert str(exc) == expected
        else:
            raise AssertionError("expected TypeError")
    print("Python os._fspath passed")
