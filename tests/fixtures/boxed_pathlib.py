import glob
import importlib
import ntpath
import os
import pathlib
import pickle
import sys
from pathlib import Path, PurePosixPath, PureWindowsPath
from urllib.request import pathname2url


def current():
    assert "pathlib.types" not in sys.modules and "types" not in vars(pathlib)
    name = ".".join((pathlib.__name__, "types"))
    path_types = importlib.import_module(name)
    assert path_types is sys.modules[name] and path_types is pathlib.types
    assert importlib.import_module(name) is path_types
    assert "pathlib._local" not in sys.modules and "_local" not in vars(pathlib)
    for flavour in (PurePosixPath, PureWindowsPath):
        legacy_global = b"cpathlib._local\n" + flavour.__name__.encode() + b"\n."
        assert pickle.loads(legacy_global) is flavour
        pure = flavour("child")
        assert isinstance(flavour.parser, path_types._PathParser)
        assert not isinstance(pure, path_types._PathParser)
        assert isinstance(pure, path_types._JoinablePath)
        assert not isinstance(pure, path_types._ReadablePath)
        assert not isinstance(pure, path_types._WritablePath)
    assert issubclass(Path, path_types._ReadablePath)
    assert issubclass(Path, path_types._WritablePath)
    assert isinstance(Path("child").info, path_types.PathInfo)
    assert not isinstance("child", path_types.PathInfo)

    assert bytes(PurePosixPath("a/b")) == b"a/b"
    assert bytes(PureWindowsPath("a/b")) == b"a\\b"

    assert PurePosixPath("a//./b/../c").parts == ("a", "b", "..", "c")
    assert PureWindowsPath(r"C:\a\.\b\..").parts == ("C:\\", "a", "b", "..")
    assert PurePosixPath("α/\udcff/./β").parts == ("α", "\udcff", "β")

    class Spelling(str):
        def split(self, separator):
            assert separator == "|"
            return ["", ".", "custom", ".."]

    class Parser:
        sep = "|"
        altsep = None

        @staticmethod
        def splitroot(path):
            return "", "", Spelling(path)

    reads = []

    class PathFunctions:
        @property
        def fspath(self):
            reads.append("fspath")
            return os.fspath

    original_os = pathlib.os
    try:
        pathlib.os = PathFunctions()
        raw = PurePosixPath("first", "second")
        copied = PurePosixPath(raw)
        empty = PurePosixPath()
    finally:
        pathlib.os = original_os
    assert reads == ["fspath", "fspath"]
    assert raw.parts == copied.parts == ("first", "second")
    assert not empty.parts

    checks = []

    class PathMeta(type):
        def __instancecheck__(cls, value):
            checks.append(value)
            return False

    class PathMarker(metaclass=PathMeta):
        pass

    original_type = pathlib.PurePath
    try:
        pathlib.PurePath = PathMarker
        assert PurePosixPath("one", "two")._raw_paths == ["one", "two"]
    finally:
        pathlib.PurePath = original_type
    assert checks == ["one", "two"]

    class CustomPath(PurePosixPath):
        parser = Parser

    assert CustomPath("original").parts == ("custom", "..")
    assert CustomPath._parse_pattern("original") == ["custom", ".."]

    path = PurePosixPath("parent")
    assert not path.joinpath("child").is_absolute()
    assert path.joinpath("/child").is_absolute()

    assert PureWindowsPath(r"C:\parent\child").drive == "C:"
    assert ntpath.splitdrive(r"C:\parent") == ("C:", r"\parent")
    assert glob.has_magic("a[b]") and not glob.has_magic("a")

    assert pathname2url("a#b c") == "a%23b%20c"
    if os.name == "posix":
        check_literal_scan()
        assert pathname2url(Path("é")) == "%C3%A9"
        assert pathname2url(os.fsdecode(b"/bad\xff")) == "///bad%FF"
        assert Path("//server/a?b").as_uri() == "file:////server/a%3Fb"
    try:
        Path("relative").as_uri()
    except ValueError:
        pass
    else:
        raise AssertionError("relative path accepted as a file URI")


def check_literal_scan():
    import shutil
    import tempfile

    root = Path(tempfile.mkdtemp())
    try:
        name = os.fsdecode(b"Report-\xff.txt")
        nested = root / "nested"
        nested.mkdir()
        top = root / name
        leaf = nested / name
        direct = root.glob(name, case_sensitive=True)
        recursive = root.rglob(name, case_sensitive=True)
        top.write_bytes(b"")
        leaf.write_bytes(b"")
        assert list(direct) == [top]
        assert set(recursive) == {top, leaf}
        assert list(root.glob(os.fsdecode(b"report-\xff.txt"), case_sensitive=True)) == []
        assert list(root.rglob("missing" + name, case_sensitive=True)) == []
        assert list(root.glob("nested/*", case_sensitive=True)) == [leaf]
    finally:
        shutil.rmtree(root)
