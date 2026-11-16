from typing import Optional as Maybe, Union


def text(value: Union[str, None, bytes]) -> str:
    if isinstance(value, str):
        return value
    if value is None:
        return "absent"
    return value.decode()


def copied(value: Maybe[bytes]) -> Maybe[str | bytes]:
    return value


def selected(flag: bool) -> str | bytes | None:
    return ("text" if flag else b"bytes") or None


def fallback(value: str | None) -> str | bytes:
    return value or b"fallback"


def inspect(value: str | bytes | None) -> None:
    print(bool(value))
    print(isinstance(value, str))
    print(isinstance(value, bytes))
    print(text(value))


cases: list[bytes | None] = [None, b"", b"payload"]
for value in cases:
    inspect(copied(value))
inspect("")
inspect("text")
inspect(selected(True))
inspect(selected(False))
print(text(fallback(None)))
print(text(fallback("")))
print(text(fallback("kept")))


class Source:
    def get(self, flag: bool) -> str | bytes | None:
        pass


class Words(Source):
    def get(self, flag: bool) -> str | bytes:
        return "text" if flag else b"bytes"


def read(source: Source, flag: bool) -> str:
    return text(source.get(flag))


print(read(Source(), True), read(Words(), True), read(Words(), False))
