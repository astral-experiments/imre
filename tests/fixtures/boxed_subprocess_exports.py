import subprocess as _subprocess
from subprocess import *


class ShellPath:
    def __fspath__(self):
        raise AssertionError("shell should reject the path before calling __fspath__")


def current():
    print(_subprocess.__all__)
    for name in _subprocess.__all__:
        assert globals()[name] is getattr(_subprocess, name)
    assert "list2cmdline" not in globals() and "STARTUPINFO" not in globals()
    assert check_output(["/bin/cat"], input=b"abc") == b"abc"
    assert check_output(["/bin/cat"], input=None, text=True) == ""
    assert getstatusoutput("printf 'out\\n\\n' >&2; exit 7") == (7, "out\n")
    assert getoutput("printf x") == "x"

    words = [b'printf "%s|%s|%s" "$TEST_VALUE" "$0" "$1"', "script name", "two words"]
    with Popen(words, shell=True, env={"TEST_VALUE": "env"}, stdout=PIPE) as child:
        assert child.args is words
        assert child.communicate() == (b"env|script name|two words", None)
        assert child.returncode == 0

    assert call(b"exit 23", shell=True) == 23
    with Popen([], shell=True, stderr=PIPE) as child:
        assert child.communicate()[1]
        assert child.returncode != 0
    try:
        Popen(ShellPath(), shell=True)
    except TypeError as exc:
        assert str(exc) == "path-like args is not allowed when shell is true"
    else:
        raise AssertionError("accepted scalar path-like shell command")

    for name in ("run", "PIPE", "STDOUT", "DEVNULL"):
        delattr(_subprocess, name)
        try:
            assert not hasattr(_subprocess, name)
        finally:
            setattr(_subprocess, name, globals()[name])
