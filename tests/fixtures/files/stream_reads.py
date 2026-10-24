import gzip
from pathlib import Path
from typing import BinaryIO, TextIO


def text_lines(stream: TextIO) -> None:
    alias = stream
    first = stream.readline()
    print(len(first))
    print(first[-2:])
    for line in alias:
        print(len(line))
        print(line[-3:])
        break
    else:
        print("unexpected exhaustion")
    print(alias.readline())
    print(any(line == "\n" for line in stream))
    print(stream.read())
    print(len(alias.readline()))
    for line in stream:
        print("unexpected data")
    else:
        print("exhausted")


def binary_lines(stream: BinaryIO) -> None:
    alias = stream
    print(len(stream.read(0)))
    print(len(alias.read(8191)))
    print(stream.read(1) == b"\r")
    print(alias.readline() == b"\n")
    for line in stream:
        print(len(line))
        print(line[-3:] == b"\rC\n")
        break
    try:
        alias.read(-2)
    except ValueError:
        print("negative size")
    print(stream.readline() == b"\n")
    print(alias.read(-1) == b"D\0")
    print(len(stream.read(9)))
    print(len(alias.readline()))


with open("long-rows.txt", "r", encoding="utf-8") as long_rows:
    kept = [line for line in long_rows]
for line in kept:
    print(len(line))
    print(repr(line[:2]))
    print(repr(line[-2:]))
with open("rows.txt", "r", encoding="utf-8") as text:
    text_lines(text)
with gzip.open(Path("rows.gz"), "rt", encoding="utf-8") as text:
    text_lines(text)
with open("rows.txt", "rb") as stream:
    binary_lines(stream)
with gzip.open("rows.gz", "rb") as stream:
    binary_lines(stream)
with gzip.open("padded.gz", "rb") as stream:
    print(len(stream.read()))
with gzip.open("empty.gz", "rt", encoding="utf-8") as text:
    print(len([line for line in text]))
    print(len(text.readline()))
with gzip.open("rows.gz", "rt", encoding="utf-8") as text:
    for line in text:
        text.close()
        break
    print(text.closed)

for suffix in [b"\xed\xa0", b"\xed\xbf", b"\xed\xa0\x80", b"\xe0\x80"]:
    with open("partial-utf8.txt", "wb") as writer:
        writer.write(b"\n" + suffix)
    with open("partial-utf8.txt", "r", encoding="utf-8") as text:
        for _ in range(2):
            try:
                print(repr(text.readline()))
            except UnicodeDecodeError:
                print("invalid UTF-8")
