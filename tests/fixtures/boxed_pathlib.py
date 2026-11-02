import glob
import ntpath
from pathlib import PurePosixPath, PureWindowsPath


def current():
    path = PurePosixPath("parent")
    assert not path.joinpath("child").is_absolute()
    assert path.joinpath("/child").is_absolute()

    assert PureWindowsPath(r"C:\parent\child").drive == "C:"
    assert ntpath.splitdrive(r"C:\parent") == ("C:", r"\parent")
    assert glob.has_magic("a[b]") and not glob.has_magic("a")
