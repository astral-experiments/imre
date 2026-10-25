from pathlib import Path
from shutil import copy, copy2, copyfile, copymode, rmtree
from typing import Callable

def copy_bytes(copier: Callable[[bytes, bytes], bytes]) -> bytes:
    return copier(b"empty.txt", b"empty-directory")

class Visitor:
    name: str

    def __init__(self, name: str) -> None:
        self.name = name

    def read(self, suffix: str) -> str:
        return self.name + suffix

def bind(name: str) -> Callable[[str], str]:
    print("bind " + name)
    return Visitor(name).read

def fail() -> Callable[[str], str]:
    raise ValueError("stop before iteration")

for copier in (copy, copy2):
    print(copier("input.txt", "empty-directory"))
    print(copier("input.txt", "copied.txt"))
print(copy_bytes(copy2))
copy_path: Callable[[str, Path], Path] = copyfile
print(copy_path("empty.txt", Path("path-result")).name)
copy_permissions: Callable[[Path, bytes], None] = copymode
print(copy_permissions(Path("input.txt"), b"copied.txt"))
remove: Callable[[Path, bool], None] = rmtree
remove(Path("nested"), False)
print(Path("nested").exists())
remove(Path("nested"), True)

for visit in (bind("first"), bind("second")):
    print(visit("!"))
    break
for visit in reversed((bind("third"), bind("fourth"))):
    print(visit("?"))
try:
    for visit in (bind("fifth"), fail()):
        print(visit("unreachable"))
except ValueError as exc:
    print(str(exc))
