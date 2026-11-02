import os
import subprocess
from pathlib import Path
from sys import argv


def marked(value: str) -> str:
    print(value)
    return value


def lookup(key: str, default: str | None) -> str | None:
    return os.getenv(key, default)


def show(value: str | None) -> None:
    if value is None:
        print("absent")
    else:
        print(value)


show(os.environ.get("IMRE_PROCESS_PARENT"))
print(os.environ.get("IMRE_PROCESS_EMPTY") is None)
print(os.getenv("IMRE_PROCESS_EMPTY", None) is None)
show(lookup("IMRE_PROCESS_PARENT", None))
show(lookup("IMRE_PROCESS_EMPTY", "fallback"))
print(os.getenv(default=marked("eager default"), key=marked("IMRE_PROCESS_PARENT")))
print(os.environ.get(default=marked("second default"), key=marked("IMRE_PROCESS_EMPTY")))
print(os.getcwd().endswith("/work"))
print(os.fspath(Path("./directory")))
print(os.fspath(b"\xff") == b"\xff")
print(os.fspath("directory/"))


command = [argv[1], "-I", argv[2], "environment"]
inherited = subprocess.run(command, capture_output=True, encoding="utf-8")
print(inherited.stdout)
first = dict(os.environ)
second = os.environ.copy()
if os.supports_bytes_environ:
    snapshot = dict(os.environb)
    snapshot[b"IMRE_PROCESS_OVERRIDE"] = b"copy only"
    assert dict(os.environb)[b"IMRE_PROCESS_OVERRIDE"] == b"parent"
print(first[""])
first["IMRE_PROCESS_OVERRIDE"] = "changed λ"
first["IMRE_PROCESS_EMPTY"] = ""
del first["IMRE_PROCESS_PARENT"]
print(second["IMRE_PROCESS_OVERRIDE"])
print(os.environ.copy()["IMRE_PROCESS_OVERRIDE"])
changed = subprocess.run(command, capture_output=True, encoding="utf-8", env=first, cwd=Path("directory"))
print(changed.stdout)
os.environ[marked("IMRE_PROCESS_OVERRIDE")] = marked("assigned λ")
print(os.environ.get("IMRE_PROCESS_OVERRIDE"))
print(subprocess.run(command, capture_output=True, encoding="utf-8").stdout)
os.environb[b"IMRE_PROCESS_OVERRIDE"] = b"bytes assignment"
print(os.environ.get("IMRE_PROCESS_OVERRIDE"))
os.environb[b"IMRE_PROCESS_NEW_BYTES"] = b"\xff"
print(dict(os.environb)[b"IMRE_PROCESS_NEW_BYTES"])
del os.environb[b"IMRE_PROCESS_NEW_BYTES"]
label = marked("IMRE_PROCESS_OVERRIDE")
print(os.environ[label])
del os.environ[label]
print(subprocess.run(command, capture_output=True, encoding="utf-8").stdout)
try:
    os.environ[label]
except KeyError as error:
    assert error.args[0] is label
    assert error.args[0] is not (label + "_")[:-1]
    assert error.__suppress_context__
try:
    del os.environ[label]
except KeyError as error:
    assert error.args[0] is label
    assert error.__suppress_context__
byte_label = label.encode("utf-8")
try:
    del os.environb[byte_label]
except KeyError as error:
    assert error.args[0] is byte_label
    assert error.__suppress_context__
for invalid in ("", "bad=name", "bad\0name"):
    try:
        os.environ[invalid] = "rejected"
    except (ValueError, OSError) as error:
        print(str(error))
try:
    os.environ["IMRE_PROCESS_OVERRIDE"] = "bad\0value"
except ValueError as error:
    print(str(error))
print(os.environ.get("IMRE_PROCESS_OVERRIDE"))

empty = subprocess.run(command, capture_output=True, encoding="utf-8", env={})
print(empty.stdout)
# An explicit cwd selects CPython's fork/exec path, which accepts an empty name.
empty_name = subprocess.run(command, capture_output=True, encoding="utf-8", env={"": "allowed"}, cwd=".")
print(empty_name.returncode)
print(empty_name.stdout)

# Executable search uses the replacement PATH after changing directory.
searched = subprocess.run(["imre-process-child", "-I", argv[2], "environment"], capture_output=True, encoding="utf-8", env={"PATH": "bin", "IMRE_PROCESS_OVERRIDE": "searched"}, cwd="directory")
print(searched.stdout)
relative = subprocess.run(["./relative-executable", "-I", argv[2], "environment"], capture_output=True, encoding="utf-8", env={}, cwd=Path("directory"))
print(relative.stdout)

optional_path: Path | None = Path("directory")
optional_text: str | None = "directory"
optional_env: dict[str, str] | None = {"IMRE_PROCESS_OVERRIDE": "optional"}
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_path, env=optional_env).stdout)
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_text, env=None).stdout)
optional_path = None
optional_text = None
optional_env = None
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_path, env=optional_env).stdout)
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_text).stdout)
