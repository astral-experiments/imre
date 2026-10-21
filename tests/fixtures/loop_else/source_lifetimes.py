# ruff: noqa: PLW0120, SIM115
# Exhaustion must release source owners before executing the else suite.

from typing import BinaryIO, TextIO


def text_writer(path: str, text: str) -> TextIO:
    writer = open(path, "w", encoding="utf-8")
    writer.write(text)
    return writer


def binary_writer(path: str) -> BinaryIO:
    writer = open(path, "wb")
    writer.write(b"binary payload")
    return writer


def list_return(path: str) -> str:
    writer: TextIO | None = None
    for writer in [text_writer(path, "list payload")]:
        assert writer is not None
        assert not writer.closed
        writer = None
        assert writer is None
    else:
        with open(path, "r", encoding="utf-8") as reader:
            return reader.read()


def binary_failure(path: str) -> None:
    writer: BinaryIO | None = None
    for writer in [binary_writer(path)]:
        assert writer is not None
        assert not writer.closed
        writer = None
        assert writer is None
    else:
        with open(path, "rb") as reader:
            raise ValueError(reader.read().decode())


def enumeration(path: str) -> None:
    writer: TextIO | None = None
    for index, writer in enumerate([text_writer(path, "enumerated payload")]):
        print(index)
        assert writer is not None
        assert not writer.closed
        writer = None
        assert writer is None
    else:
        with open(path, "r", encoding="utf-8") as reader:
            print(reader.read())


def items(path: str) -> None:
    writer: TextIO | None = None
    for key, writer in {"item": text_writer(path, "dictionary payload")}.items():
        print(key)
        assert writer is not None
        assert not writer.closed
        writer = None
        assert writer is None
    else:
        with open(path, "r", encoding="utf-8") as reader:
            print(reader.read())


print(list_return("list.txt"))
try:
    binary_failure("binary.bin")
except ValueError as error:
    print(error)
enumeration("enumeration.txt")
items("dictionary.txt")
