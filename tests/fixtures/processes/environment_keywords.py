import os
import subprocess


def watch(source: dict[str, str]) -> str:
    assert os.environ.get("HELLO") == "before"
    assert os.environ.get("POSITION") is None
    source["POSITION"] = "set"
    return "World"


def fail() -> str:
    raise ValueError("argument")


saved = dict(os.environb)
os.environ.clear()
os.environ.update(HELLO="World")
command = ["/bin/sh", "-c", "echo $HELLO"]
assert subprocess.run(command, encoding="utf-8", capture_output=True, check=True).stdout.strip() == "World"
os.environ.update(HELLO="before")
source = {"HELLO": "first"}
try:
    os.environ.update(source, HELLO=watch(source), LATE=fail())
except ValueError:
    pass
else:
    raise AssertionError("argument exception missing")
assert os.environ.get("HELLO") == "before"
assert os.environ.get("POSITION") is None
source = {"HELLO": "first"}
os.environ.update(source, HELLO=watch(source))
assert os.environ.get("POSITION") == "set"
assert subprocess.run(command, encoding="utf-8", capture_output=True, check=True).stdout.strip() == "World"
try:
    os.environ.update({"HELLO": "positional"}, FIRST="yes", BAD="NUL\0", LATE="unset")
except ValueError:
    pass
else:
    raise AssertionError("invalid value accepted")
assert os.environ.get("HELLO") == "positional"
assert os.environ.get("FIRST") == "yes"
assert os.environ.get("BAD") is None
assert os.environ.get("LATE") is None
os.environb.clear()
os.environb.update(saved)
assert dict(os.environb) == saved
print("keyword updates")
