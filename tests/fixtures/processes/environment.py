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


show(os.environ.get("RELAR_PROCESS_PARENT"))
print(os.environ.get("RELAR_PROCESS_EMPTY") is None)
print(os.getenv("RELAR_PROCESS_EMPTY", None) is None)
show(lookup("RELAR_PROCESS_PARENT", None))
show(lookup("RELAR_PROCESS_EMPTY", "fallback"))
print(os.getenv(default=marked("eager default"), key=marked("RELAR_PROCESS_PARENT")))
print(os.environ.get(default=marked("second default"), key=marked("RELAR_PROCESS_EMPTY")))
print(os.getcwd().endswith("/work"))
print(os.fspath(Path("./directory")))
print(os.fspath(b"\xff") == b"\xff")
print(os.fspath("directory/"))


command = [argv[1], "-I", argv[2], "environment"]
inherited = subprocess.run(command, capture_output=True, encoding="utf-8")
print(inherited.stdout)
first = os.environ.copy()
second = os.environ.copy()
print(first[""])
first["RELAR_PROCESS_OVERRIDE"] = "changed λ"
first["RELAR_PROCESS_EMPTY"] = ""
del first["RELAR_PROCESS_PARENT"]
print(second["RELAR_PROCESS_OVERRIDE"])
print(os.environ.copy()["RELAR_PROCESS_OVERRIDE"])
changed = subprocess.run(command, capture_output=True, encoding="utf-8", env=first, cwd=Path("directory"))
print(changed.stdout)
empty = subprocess.run(command, capture_output=True, encoding="utf-8", env={})
print(empty.stdout)
# An explicit cwd selects CPython's fork/exec path, which accepts an empty name.
empty_name = subprocess.run(command, capture_output=True, encoding="utf-8", env={"": "allowed"}, cwd=".")
print(empty_name.returncode)
print(empty_name.stdout)

# Executable search uses the replacement PATH after changing directory.
searched = subprocess.run(["relar-process-child", "-I", argv[2], "environment"], capture_output=True, encoding="utf-8", env={"PATH": "bin", "RELAR_PROCESS_OVERRIDE": "searched"}, cwd="directory")
print(searched.stdout)
relative = subprocess.run(["./relative-executable", "-I", argv[2], "environment"], capture_output=True, encoding="utf-8", env={}, cwd=Path("directory"))
print(relative.stdout)

optional_path: Path | None = Path("directory")
optional_text: str | None = "directory"
optional_env: dict[str, str] | None = {"RELAR_PROCESS_OVERRIDE": "optional"}
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_path, env=optional_env).stdout)
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_text, env=None).stdout)
optional_path = None
optional_text = None
optional_env = None
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_path, env=optional_env).stdout)
print(subprocess.run(command, capture_output=True, encoding="utf-8", cwd=optional_text).stdout)
