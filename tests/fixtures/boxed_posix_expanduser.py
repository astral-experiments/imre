import os
import pwd
import sys
from os.path import expanduser as os_expanduser
from posixpath import expanduser


def current():
    assert expanduser is os_expanduser is os.path.expanduser
    account = pwd.getpwuid(os.getuid())
    named = "~" + account.pw_name
    home = account.pw_dir.rstrip("/") or "/"
    names = {entry.pw_name for entry in pwd.getpwall()}
    missing = "imre_missing_account"
    while missing in names:
        missing += "_"

    old_home = os.environ.pop("HOME", None)
    try:
        assert expanduser("~") == home
        os.environ["HOME"] = "/overridden-home///"
        assert expanduser("~/file") == "/overridden-home/file"
        assert expanduser(named) == home
        assert expanduser(os.fsencode(named)) == os.fsencode(home)
        assert expanduser("~" + missing + "/file") == "~" + missing + "/file"
        raw = b"\xc3\xa9" + missing.encode("ascii")
        decoded = os.fsdecode(raw)
        if sys.getfilesystemencoding() == "ascii":
            assert list(map(ord, decoded[:2])) == [0xdcc3, 0xdca9]
        assert os.fsencode(decoded) == raw
        while os.fsdecode(raw) in names:
            raw += b"_"
        assert expanduser(b"~" + raw) == b"~" + raw
        os.environ["HOME"] = ""
        assert expanduser(b"~") == b"/"
    finally:
        if old_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = old_home
    print("POSIX home expansion passed")


if __name__ == "__main__":
    current()
