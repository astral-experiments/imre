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

    path = PurePosixPath("parent")
    assert not path.joinpath("child").is_absolute()
    assert path.joinpath("/child").is_absolute()

    assert PureWindowsPath(r"C:\parent\child").drive == "C:"
    assert ntpath.splitdrive(r"C:\parent") == ("C:", r"\parent")
    assert glob.has_magic("a[b]") and not glob.has_magic("a")

    assert pathname2url("a#b c") == "a%23b%20c"
    if os.name == "posix":
        assert pathname2url(Path("é")) == "%C3%A9"
        assert pathname2url(os.fsdecode(b"/bad\xff")) == "///bad%FF"
        assert Path("//server/a?b").as_uri() == "file:////server/a%3Fb"
    try:
        Path("relative").as_uri()
    except ValueError:
        pass
    else:
        raise AssertionError("relative path accepted as a file URI")
