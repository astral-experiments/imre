import glob
import ntpath
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
from pathlib.types import PathInfo
from urllib.request import pathname2url


def current():
    assert isinstance(Path(".").info, PathInfo)
    assert not isinstance(object(), PathInfo)

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
