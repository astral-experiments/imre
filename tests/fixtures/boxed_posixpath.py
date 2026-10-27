import os
import os.path
import posix
import posixpath
from os import fspath


class Path:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        return self.value


class Text(str):
    def __fspath__(self):
        raise AssertionError("string paths bypass __fspath__")


class Raw(bytes):
    def __fspath__(self):
        raise AssertionError("byte paths bypass __fspath__")


class Descriptor:
    def __get__(self, obj, owner):
        raise RuntimeError("descriptor lookup")


class NoMethod:
    __fspath__ = Descriptor()


class Raises:
    def __fspath__(self):
        raise RuntimeError("method call")


def current():
    assert os.path is posixpath and os.path.__name__ == "posixpath"
    assert os.path.splitext is posixpath.splitext and os.path.join is posixpath.join
    assert fspath is os.fspath is posix.fspath
    assert fspath.__self__ is posix and fspath.__module__ == "posix"
    for value in ("", ".profile", "a.b/.c", "a/.a.b", "/résumé.☃", "r/", b"a.\xff", Text("a.tar.gz"), Raw(b"a.tar.gz")):
        path = Path(value)
        assert fspath(path) is value and fspath(value) is value
        assert os.path.normcase(path) is value
        drive, tail = os.path.splitdrive(path)
        assert tail is value
        print(drive, os.path.basename(path), os.path.splitext(path))
    class ChangedSearch(str):
        def rfind(self, sep):
            if sep == "/":
                return None
            raise LookupError("dot search")

    try:
        posixpath.splitext(ChangedSearch("a.b"))
    except LookupError as error:
        print(str(error))
    else:
        raise AssertionError("the extension search must run")

    for value in (NoMethod(), Raises(), Path(42), None):
        try:
            fspath(value)
        except (TypeError, RuntimeError) as error:
            print(type(error).__name__, str(error))
    original = os.fspath
    calls = []
    def counted(value):
        calls.append(value)
        return original(value)
    try:
        os.fspath = counted
        for _ in range(3):
            os.path.normcase("werkzeug/serving.py")
        filename = os.path.basename("werkzeug/serving.py")
        assert os.path.splitext(filename) == ("serving", ".py")
    finally:
        os.fspath = original
    assert calls == ["werkzeug/serving.py"] * 4 + ["serving.py"]

    assert os.fsdecode(Path(b"caf\xc3\xa9")) == "café"
    assert not os.path.isfile(Path("missing\0file"))
