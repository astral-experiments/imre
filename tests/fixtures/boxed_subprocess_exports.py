import subprocess as _subprocess
from subprocess import *


def current():
    print(_subprocess.__all__)
    for name in _subprocess.__all__:
        assert globals()[name] is getattr(_subprocess, name)
    assert "list2cmdline" not in globals() and "STARTUPINFO" not in globals()
    assert check_output(["/bin/cat"], input=b"abc") == b"abc"
    assert check_output(["/bin/cat"], input=None, text=True) == ""
    assert getstatusoutput("printf 'out\\n\\n' >&2; exit 7") == (7, "out\n")
    assert getoutput("printf x") == "x"
