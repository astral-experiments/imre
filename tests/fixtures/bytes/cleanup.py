# ruff: noqa: SIM115
# Manual closure and rebinding exercise descriptor ownership.

from typing import BinaryIO


def temporary(label: str) -> bytes:
    print(label)
    return bytes([0, 128, 255])


def failing() -> bytes:
    print("failure")
    raise ValueError("stop")


def join(left: bytes, right: bytes) -> bytes:
    return left + right


def write_and_return(stream: BinaryIO, payload: bytes) -> bytes:
    with stream as visible:
        print(visible.write(payload))
        visible = open("replacement.bin", "wb")
        visible.write(b"replacement")
        visible.close()
        return payload[::-1]


def next_payload(payload: bytes, fail: bool) -> bytes:
    if fail:
        raise ValueError("partial")
    return payload[::-1]


class Partial:
    payload: bytes
    next: bytes

    def __init__(self, payload: bytes, fail: bool) -> None:
        self.payload = payload
        self.next = next_payload(payload, fail)


for index in range(20):
    try:
        print(join(temporary("first"), failing()))
    except ValueError:
        print("recovered")
    try:
        partial = Partial(temporary("partial"), True)
    except ValueError:
        print("partial recovered")
    try:
        converted = [value.decode() for value in [b"ok", b"\xff"]]
    except UnicodeDecodeError:
        print("decode recovered")
    try:
        data = [bytes([value]) for value in [1, 2, 256]]
    except ValueError:
        print("constructor recovered")

stream = open("return.bin", "wb")
print(write_and_return(stream, temporary("return")))
print(stream.closed)
outer = open("outer.bin", "wb")
try:
    with outer, open("missing.bin", "rb"):
        print("unreachable")
except FileNotFoundError:
    print("later manager failed")
print(outer.closed)
loop = open("loop.bin", "ab")
loop.close()
for index in range(3):
    with open("loop.bin", "ab") as loop:
        loop.write(bytes([index]))
        if index == 0:
            continue
        break
print(loop.closed)
error = open("error.bin", "wb")
try:
    with error:
        error.write(b"saved\0\xff")
        raise ValueError("body")
except ValueError:
    print("body failed")
print(error.closed)
