import importlib

subprocess = importlib.import_module("subprocess")


class ShellPath:
    def __fspath__(self):
        raise AssertionError("shell should reject the path before calling __fspath__")


words = [b'printf "%s|%s|%s" "$TEST_VALUE" "$0" "$1"', "script name", "two words"]
with subprocess.Popen(words, shell=True, env={"TEST_VALUE": "env"}, stdout=subprocess.PIPE) as child:
    assert child.args is words
    assert child.communicate() == (b"env|script name|two words", None)
    assert child.returncode == 0

assert subprocess.call(b"exit 23", shell=True) == 23
with subprocess.Popen([], shell=True, stderr=subprocess.PIPE) as child:
    assert child.communicate()[1]
    assert child.returncode != 0
try:
    subprocess.Popen(ShellPath(), shell=True)
except TypeError as exc:
    assert str(exc) == "path-like args is not allowed when shell is true"
else:
    raise AssertionError("accepted scalar path-like shell command")
