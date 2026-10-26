from enum import StrEnum
from os import fsdecode, fsencode, fspath
from pathlib import Path


class PathValue:
    value: str | bytes
    calls: int

    def __init__(self, value: str | bytes) -> None:
        self.value = value
        self.calls = 0

    def __fspath__(self) -> str | bytes:
        self.calls += 1
        return self.value


class Invalid:
    calls: int

    def __init__(self) -> None:
        self.calls = 0

    def __fspath__(self) -> int:
        self.calls += 1
        return 17


class Refused:
    calls: int

    def __init__(self) -> None:
        self.calls = 0

    def __fspath__(self) -> str:
        self.calls += 1
        raise OSError("path refused")


class Name(StrEnum):
    upload = "upload"


class EnumPath:
    def __fspath__(self) -> Name:
        return Name.upload


text = "café/🧪.txt"
raw = b"caf\xc3\xa9/\xf0\x9f\xa7\xaa.txt"
assert fsdecode(filename=text) is text
assert fsencode(filename=raw) is raw
assert fsdecode(Name.upload) is Name.upload
assert fsdecode(EnumPath()) is Name.upload
assert fsencode(text) == raw
assert fsdecode(raw) == text
assert fsdecode(b"x\x00y") == "x\x00y"
assert fsdecode(b"") == ""
opaque = b"actual-\xff.bin"
assert fsencode(opaque) is opaque
native_path = Path(text)
assert fsdecode(native_path) is fspath(native_path)
assert fsencode(native_path) == raw
text_path = PathValue(text)
assert fsdecode(text_path) is text
assert text_path.calls == 1
assert fsencode(text_path) == raw
assert text_path.calls == 2
byte_path = PathValue(raw)
assert fsencode(byte_path) is raw
assert fsdecode(byte_path) == text
assert byte_path.calls == 2

bad = Invalid()
try:
    fsdecode(bad)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("bad path result accepted")
assert bad.calls == 1
refused = Refused()
try:
    fsencode(refused)
except OSError as exc:
    print(str(exc))
else:
    raise AssertionError("path exception lost")
assert refused.calls == 1
try:
    fsdecode(17)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("bad filename accepted")
try:
    fsencode(raw, filename=text)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("duplicate filename accepted")
