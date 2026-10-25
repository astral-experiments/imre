"""Compare native ignore callbacks and returned pattern functions with CPython."""
import os
import subprocess
import sys
from tree_callback_controls import snapshot


SOURCE = r'''import shutil
import sys

def skip(path: str, names: list[str]) -> set[str]:
    print(repr((path, names)))
    return {"ignored", "trim"}

def raw(path: bytes, names: list[bytes]) -> list[bytes]:
    print(repr((path, names)))
    return [b"raw-\xff"]

def reject(path: str, names: list[str]) -> list[str]:
    raise OSError("cannot start")

def nested(path: str, names: list[str]) -> list[str]:
    if path.endswith("/sub"):
        raise OSError("cannot inspect sub")
    return []

class Shared:
    ignored: list[str]
    def __init__(self) -> None:
        self.ignored = []
    def names(self, path: str, names: list[str]) -> list[str]:
        return self.ignored
    def copy(self, src: str, dst: str) -> str:
        self.ignored.extend(["first", "ignored", "trim", "last"])
        return shutil.copyfile(src, dst)

mode = sys.argv[1]
shared = Shared()
try:
    if mode == "set":
        shutil.copytree("src", "dst", ignore=skip)
    elif mode == "raw":
        shutil.copytree(b"src", b"dst", ignore=raw)
    elif mode == "early":
        shutil.copytree("src", "dst", ignore=reject)
    elif mode == "nested":
        shutil.copytree("src", "dst", ignore=nested)
    elif mode == "mutable":
        shutil.copytree("src", "dst", ignore=shared.names, copy_function=shared.copy)
    elif mode == "raw-patterns":
        p = shutil.ignore_patterns(b"raw-[\x80-\xff]", b"t*")
        print(sorted(p(b"unused", [b"raw-\xff", b"raw-a", b"trim", b"raw-\xff"])))
        shutil.copytree(b"src", b"dst", ignore=p)
    else:
        pattern = shutil.ignore_patterns("*.tmp", "t?i[!x]", "sub*")
        print(sorted(pattern("unused", ["trim", "trix", "a.tmp", "sub", "a.tmp", "stay"])))
        empty = shutil.ignore_patterns()
        print(len(empty("unused", ["ignored"])))
        shutil.copytree("src", "dst", ignore=pattern)
except shutil.Error as exc:
    print(repr(exc))
except OSError as exc:
    print(repr(exc))
'''


def run(tools, work):
    work.mkdir()
    native = tools.build(SOURCE.encode(), work)
    for mode in ("set", "raw", "early", "nested", "mutable", "raw-patterns", "patterns"):
        complete = []
        for label, command in (("cpython", [sys.executable, "-IB", work / "case.py"]), ("imre", [native])):
            root = work / (mode + "-" + label)
            (root / "src/sub").mkdir(parents=True)
            for path in ("first", "ignored", "trim", "last", "a.tmp", "sub/foo", "sub/trim"):
                (root / "src" / path).write_text(path)
            if mode.startswith("raw"):
                with open(os.fsencode(root / "src") + b"/raw-\xff", "wb") as stream:
                    stream.write(b"arbitrary bytes")
            child = subprocess.run([*command, mode], cwd=root, capture_output=True, check=True, timeout=30)
            complete.append((child.stdout, child.stderr, (root / "dst").exists(), snapshot(root / "dst")))
        assert complete[0] == complete[1], (mode, complete)
