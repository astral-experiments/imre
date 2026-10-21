from typing import Union as Either
import typing


def text(value: Either[bytes, str]) -> str:
    if isinstance(value, str):
        result = value
    elif isinstance(value, bytes):
        result = value.decode()
    return result


def select(flag: bool) -> typing.Union[str, bytes, str]:
    return "text" if flag else b"bytes"


def quoted(value: "str | bytes") -> str:
    if not isinstance(value, bytes):
        return value
    return value.decode()


def defaulted(value: str | bytes = b"default") -> str:
    return text(value)


def optional_default(value: str | bytes | None = "optional default") -> str:
    if value is None:
        return "none"
    return text(value)


for flag in [True, False]:
    print(text(select(flag)))
    print(quoted(select(flag)))
print(text("é🐍"))
print(text(b"\xc3\xa9\xf0\x9f\x90\x8d"))
print(isinstance(select(True), (str, bytes)))
print(isinstance(select(False), (int, (str, bytes))))
print(isinstance(b"", ()))
print(defaulted())
print(defaulted(value="keyword"))
print(optional_default())
print(optional_default(None))

value: bytes | str = b"known"
if isinstance(value, str):
    print(value.encode())
elif isinstance(value, bytes):
    print(value.decode())
