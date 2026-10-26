import shlex
import subprocess
from pathlib import Path
from subprocess import list2cmdline as quote


class Argument:
    def __init__(self, label: str):
        self.label = label

    def __fspath__(self) -> bytes:
        print("path " + self.label)
        if self.label == "stop":
            raise ValueError("stop before the next argument")
        return self.label.encode()


def constructed(label: str) -> Argument:
    print("construct " + label)
    return Argument(label)


def from_arguments(words: list[str]) -> str:
    return subprocess.list2cmdline(seq=words)


print(quote([]) + "|" + quote(()) + "|" + quote(b""))
print(from_arguments(["", " "]))
for length in range(6):
    slashes = "\\" * length
    print(from_arguments([slashes + '"', "x " + slashes, slashes, "x" + slashes + "y"]))
print(quote(["a\tb", "a\nb", "a\rb", "a\0b", "雪🙂"]))
print(quote("a雪🙂"))
print(quote((b"caf\xc3\xa9", Path("two words"), "last")))
print(quote([b"first", Path("two words")]))
print(quote({"first": 1, "two words": 2}))
print(quote((value for value in ["hello", "with space", ""])))
print(quote([constructed("first"), constructed("two words")]))
try:
    print(quote([constructed("stop"), constructed("unused")]))
except ValueError as error:
    print(str(error))
try:
    print(quote(b"one"))
except TypeError as error:
    print(str(error))
try:
    print(quote(Path("alone")))
except TypeError as error:
    print(str(error))
try:
    print(quote(seq=["one"], extra=2))
except TypeError as error:
    print(str(error))

# The same iteration machinery also serves ordinary joining and POSIX shell quoting.
print(" / ".join(["a b", ""]))
print(shlex.join(["a b", ""]))
