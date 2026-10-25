import os
import os.path
import posixpath
from pathlib import Path, PurePosixPath


class TextPath:
    value: str
    calls: int

    def __init__(self, value: str) -> None:
        self.value = value
        self.calls = 0

    def __fspath__(self) -> str:
        self.calls += 1
        return self.value


class BytePath:
    value: bytes

    def __init__(self, value: bytes) -> None:
        self.value = value

    def __fspath__(self) -> bytes:
        return self.value


class BadResult:
    calls: int

    def __init__(self) -> None:
        self.calls = 0

    def __fspath__(self) -> int:
        self.calls += 1
        return 17


class BrokenPath:
    def __fspath__(self) -> str:
        raise ValueError("from __fspath__")


def preserves_spelling(value: str | bytes) -> bool:
    return posixpath.normcase(value) is value


def text_spelling(value: str | TextPath) -> str:
    return os.path.normcase(value)


def optional_spelling(value: str | None) -> str:
    return os.path.normcase(value)  # ty: ignore[no-matching-overload]


def spelling_after(seen: list[str], label: str) -> str:
    seen.append(label)
    return label


spelling = "FoO/./BaR\\../\0ß"
raw = b"FoO/./BaR\\../\0\xff"
assert preserves_spelling(spelling)
assert preserves_spelling(raw)
text_path = TextPath(spelling)
assert os.path.normcase(s=text_path) is spelling
assert text_path.calls == 1
assert text_spelling(spelling) is spelling
assert text_spelling(text_path) is spelling
assert text_path.calls == 2
assert optional_spelling(spelling) is spelling
assert posixpath.normcase(BytePath(raw)) is raw
assert os.fspath(path=raw) is raw
assert posixpath.normcase(spelling) is spelling
assert os.path.normcase(raw) is raw
native_path = Path("A/../B")
assert posixpath.normcase(native_path) == "A/../B"
assert posixpath.normcase(native_path) is os.fspath(native_path)
assert posixpath.normcase(PurePosixPath("A/../B")) == "A/../B"
assert os.path.normcase("P") != os.path.normcase("p")
assert os.path.normcase("\\") != os.path.normcase("/")

bad = BadResult()
try:
    posixpath.normcase(bad)  # ty: ignore[no-matching-overload]
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("invalid __fspath__ result")
assert bad.calls == 1

try:
    posixpath.normcase(BrokenPath())
except ValueError as exc:
    print(str(exc))
else:
    raise AssertionError("__fspath__ exception lost")

try:
    optional_spelling(None)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("invalid argument")

calls: list[str] = []


try:
    os.path.normcase(spelling_after(calls, "one"), s=spelling_after(calls, "two"))  # ty: ignore[no-matching-overload]
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("duplicate argument accepted")
assert len(calls) == 2
assert calls[0] == "one"
assert calls[1] == "two"
