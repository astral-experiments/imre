import os
import pwd
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
