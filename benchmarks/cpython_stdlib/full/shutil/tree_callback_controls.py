"""Run native copy_function values, with real files and original exception propagation."""
import os
import stat
import subprocess
import sys


SOURCE = '''import shutil
import sys

def inspect(src: str, dst: str) -> None:
    print(repr((src, dst)))

def copy(src: str, dst: str) -> str:
    return shutil.copyfile(src, dst)

def raw(src: bytes, dst: bytes) -> bytes:
    print(repr((src, dst)))
    return shutil.copyfile(src, dst)

def buffered(src: str, dst: str) -> list[str]:
    return [src, dst]

def number(src: str, dst: str) -> int:
    return int("1000000000000000000000000000")

def floating(src: str, dst: str) -> float:
    return 1.25

def boolean(src: str, dst: str) -> bool:
    return src != dst

def denied(src: str, dst: str) -> None:
    if src.endswith("/bad"):
        raise OSError("copy denied by callback")
    shutil.copyfile(src, dst)

def stop(src: str, dst: str) -> None:
    raise ValueError("callback stopped")

def nested(src: str, dst: str) -> None:
    shutil.copytree("other", "other-dst", dirs_exist_ok=True)

mode = sys.argv[1]
try:
    if mode == "inspect":
        shutil.copytree("src", "dst", symlinks=True, copy_function=inspect)
    elif mode == "copy":
        shutil.copytree("src", "dst", copy_function=copy)
    elif mode == "raw":
        shutil.copytree(b"src", b"dst", copy_function=raw)
    elif mode == "list":
        shutil.copytree("src", "dst", copy_function=buffered)
    elif mode == "number":
        shutil.copytree("src", "dst", copy_function=number)
    elif mode == "float":
        shutil.copytree("src", "dst", copy_function=floating)
    elif mode == "bool":
        shutil.copytree("src", "dst", copy_function=boolean)
    elif mode == "denied":
        shutil.copytree("src", "dst", copy_function=denied)
    elif mode == "stop":
        shutil.copytree("src", "dst", copy_function=stop)
    else:
        shutil.copytree("src", "dst", copy_function=nested)
except shutil.Error as exc:
    print(repr(exc))
except ValueError as exc:
    print(repr(exc))
'''


def snapshot(folder):
    rows = []
    for path, dirs, files in os.walk(os.fsencode(folder)):
        for name in dirs + files:
            full = os.path.join(path, name)
            mode = os.lstat(full).st_mode
            if stat.S_ISREG(mode):
                with open(full, "rb") as stream:
                    data = stream.read()
            elif stat.S_ISLNK(mode):
                data = os.readlink(full)
            else:
                data = None
            rows.append((os.path.relpath(full, os.fsencode(folder)), stat.S_IFMT(mode), data))
    return sorted(rows)


def run(tools, work):
    work.mkdir()
    native = tools.build(SOURCE.encode(), work)
    for mode in ("inspect", "copy", "raw", "list", "number", "float", "bool", "denied", "stop", "nested"):
        pairs = []
        for label, command in (("cpython", [sys.executable, "-IB", work / "case.py"]),
                               ("imre", [native])):
            folder = work / (mode + "-" + label)
            (folder / "src/sub").mkdir(parents=True)
            (folder / "src/bad").write_bytes(b"prefix")
            (folder / "src/sub/ok").write_bytes(b"sibling")
            os.symlink("bad", folder / "src/link")
            if mode == "raw":
                with open(os.fsencode(folder / "src") + b"/raw-\xff", "wb") as stream:
                    stream.write(b"raw bytes name")
            if mode == "nested":
                (folder / "other").mkdir()
                os.mkfifo(folder / "other/pipe")
            child = subprocess.run([*command, mode], cwd=folder, capture_output=True, check=True, timeout=30)
            pairs.append((child.stdout, child.stderr, snapshot(folder / "dst")))
        assert pairs[0] == pairs[1], (mode, pairs)
