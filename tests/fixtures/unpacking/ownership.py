# ruff: noqa: F841, PIE800, PLW0128, SIM115
# Buffered files expose temporary ownership and cleanup when assignment fails.

from typing import TextIO


class FileBox:
    output: TextIO

    def __init__(self, output: TextIO) -> None:
        self.output = output


def buffered(path: str) -> TextIO:
    output = open(path, "w", encoding="utf-8")
    output.write("buffered " + path)
    return output


def fail() -> TextIO:
    raise ValueError("later file")


def successful() -> None:
    first, *rest = [buffered("first.txt"), buffered("rest.txt")]
    copied = [first, *rest]
    copied[0].write(" first")
    copied[1].write(" rest")


def rejected_unpack() -> None:
    try:
        first, second = [buffered("short.txt")]
    except ValueError:
        print("short files")
        observe("short.txt")


def rejected_display() -> None:
    try:
        files = [buffered("prefix.txt"), *[], fail()]
    except ValueError:
        print("failed file display")
        observe("prefix.txt")


def rejected_mapping() -> None:
    try:
        files = {**{"prefix": buffered("mapping.txt")}, "failed": fail()}
    except ValueError:
        print("failed file mapping")
        observe("mapping.txt")


def observe(path: str) -> int:
    with open(path, "r", encoding="utf-8") as source:
        print(source.read())
    return 0


def repeated_target() -> None:
    files = [buffered("discarded.txt")]
    held, held, files[observe("one.txt")] = (
        buffered("one.txt"),
        buffered("two.txt"),
        buffered("three.txt"),
    )
    held.write(" retained")
    files[0].write(" indexed")


def chained_targets() -> None:
    held = buffered("chain-old.txt")
    files = [buffered("chain-discarded.txt")]
    held = files[observe("chain-old.txt")] = buffered("chain-new.txt")
    held.write(" first alias")
    files[0].write(" second alias")


def replace_alias(box: FileBox) -> FileBox:
    box.output = buffered("replacement.txt")
    observe("box-old.txt")
    # The shared RHS remains buffered while a receiver replaces its first alias.
    observe("chain-shared.txt")
    return box


def chained_root() -> None:
    box = FileBox(buffered("box-old.txt"))
    box.output = replace_alias(box).output = buffered("chain-shared.txt")
    box.output.write(" retained RHS")


successful()
rejected_unpack()
rejected_display()
rejected_mapping()
repeated_target()
chained_targets()
chained_root()
for path in [
    "first.txt",
    "rest.txt",
    "short.txt",
    "prefix.txt",
    "mapping.txt",
    "discarded.txt",
    "one.txt",
    "two.txt",
    "three.txt",
    "chain-old.txt",
    "chain-discarded.txt",
    "chain-new.txt",
    "box-old.txt",
    "replacement.txt",
    "chain-shared.txt",
]:
    with open(path, "r", encoding="utf-8") as source:
        print(source.read())
