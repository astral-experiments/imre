import io
import subprocess
from typing import BinaryIO


def binary_class(stream: BinaryIO) -> None:
    print(isinstance(stream, io.IOBase))
    print(isinstance(stream, io.RawIOBase))
    print(isinstance(stream, io.BufferedIOBase))
    print(isinstance(stream, io.TextIOBase))
    print(isinstance(stream, (io.BufferedIOBase, io.RawIOBase)))


def maybe_raw(stream: BinaryIO | None) -> bool:
    return isinstance(stream, io.RawIOBase)


def maybe_buffered(stream: BinaryIO | None) -> bool:
    return isinstance(stream, (str, io.BufferedIOBase))


for size in (-1, 0, 8):
    child = subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, bufsize=size)
    binary_class(child.stdin)
    binary_class(child.stdout)
    binary_class(child.stderr)
    print(maybe_raw(child.stdout))
    print(maybe_buffered(child.stdout))
    child.stdin.close()
    child.stdout.close()
    child.stderr.close()
    binary_class(child.stdout)
    print(child.wait())

with subprocess.Popen(["/bin/cat"], stdin=subprocess.PIPE, text=True, bufsize=0) as text_child:
    print(isinstance(text_child.stdin, io.TextIOBase))
    print(isinstance(text_child.stdin, io.IOBase))
    print(isinstance(text_child.stdin, (io.BufferedIOBase, io.RawIOBase)))
    text_child.stdin.close()

binary_class(io.BytesIO(b"data"))
print(maybe_raw(None))
print(maybe_buffered(None))
print(isinstance("text", (str, io.BufferedIOBase)))
print(isinstance(123, io.IOBase))

memory = io.StringIO("text")
for closed in (False, True):
    if closed:
        memory.close()
    print(isinstance(memory, io.IOBase))
    print(isinstance(memory, io.TextIOBase))
    print(isinstance(memory, (io.RawIOBase, io.BufferedIOBase)))
