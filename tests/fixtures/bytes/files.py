from io import UnsupportedOperation
from typing import BinaryIO


class Streams:
    writer: BinaryIO

    def __init__(self, writer: BinaryIO) -> None:
        self.writer = writer


def write(value: BinaryIO, payload: bytes) -> None:
    print(value.write(payload))
    value.flush()


def read(path: str) -> bytes:
    with open(path, "rb") as stream:
        value = stream.read()
        print(stream.read())
        stream.flush()
        return value


def unsupported(value: BinaryIO) -> None:
    try:
        print(value.read())
    except UnsupportedOperation as error:
        print(error)


def closed(value: BinaryIO) -> None:
    try:
        print(value.write(b""))
    except ValueError as error:
        print(error)
    try:
        value.flush()
    except ValueError as error:
        print(error)
    try:
        with value:
            print("unreachable")
    except ValueError as error:
        print(error)


payload = bytes([value for value in range(256)]) + b"\r\n\r\0"
with open("payload.bin", "xb") as output:
    streams = Streams(output)
    aliases: list[BinaryIO] = [output, streams.writer]
    print(output.closed)
    write(aliases[0], payload)
    print(read("payload.bin") == payload)
    unsupported(output)
print(output.closed)
print(aliases[1].closed)
unsupported(output)
closed(output)
output.close()
try:
    with open("payload.bin", "xb"):
        print("unreachable")
except FileExistsError as error:
    print(error)
try:
    with open("payload.bin", "xb"):
        print("unreachable")
except OSError:
    print("exclusive OSError parent")
with open("payload.bin", "ab") as append:
    print(append.write(b"\xfftail"))
print(read("payload.bin"))
with open("payload.bin", "wb") as truncate:
    print(truncate.write(b"new\r\n"))
print(read("payload.bin"))
with open("append-created.bin", "ab") as created:
    print(created.write(b"created"))
for size in [8192, 131071, 131072]:
    with open("buffered.bin", "wb") as writer:
        writer.write(bytes(size))
        print(len(read("buffered.bin")))
        writer.write(b"x")
        print(len(read("buffered.bin")))
        writer.flush()
        print(len(read("buffered.bin")))
for size in [262144, 262145]:
    with open("read-ahead.bin", "wb") as writer:
        writer.write(b"A" * (size + 1))
    with open("read-ahead.bin", "rb") as reader:
        reader.read(size)
        with open("read-ahead.bin", "wb"):
            pass
        print(reader.read(1))
with open("payload.bin", "rb") as reader:
    try:
        print(reader.write(b"wrong"))
    except UnsupportedOperation as error:
        print(error)
try:
    print(reader.read())
except ValueError as error:
    print(error)
try:
    print(reader.write(b"wrong"))
except UnsupportedOperation as error:
    print(error)
try:
    reader.flush()
except ValueError as error:
    print(error)
try:
    with open("./missing'\nλ.bin", "rb"):
        print("unreachable")
except FileNotFoundError as error:
    print(error)
try:
    with open("bad\0path", "wb"):
        print("unreachable")
except ValueError as error:
    print(error)
try:
    with open(".", "rb"):
        print("unreachable")
except IsADirectoryError as error:
    print(error)
try:
    with open("payload.bin/child", "wb"):
        print("unreachable")
except NotADirectoryError as error:
    print(error)
