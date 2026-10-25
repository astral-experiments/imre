import io
from typing import TextIO


def capture(stream: TextIO, text: str) -> int:
    return stream.write(text)


def show_lines(stream: io.StringIO) -> None:
    print(repr(stream.getvalue()))
    print(stream.tell())
    print(repr(stream.read(1)))
    print(repr(stream.readline(2)))
    for line in stream:
        print(repr(line))
    print(repr(stream.read(None)))


def newline_text() -> str:
    print("newline first")
    return "\r\n"


def initial_text() -> str:
    print("initial second")
    return "first\n"


with io.StringIO("λ🙂\0\r\nnext\rfinal\n") as memory:
    show_lines(memory)
    print(memory.seek(1))
    print(capture(memory, "é"))
    snapshot = memory.getvalue()
    print(memory.seek(0, 2))
    print(memory.write("!"))
    print(repr(snapshot))
    print(repr(memory.getvalue()))
    print(memory.truncate(3))
    print(memory.tell())
    print(repr(memory.getvalue()))
    print(memory.write(""))
    print(repr(memory.getvalue()))
    print(memory.write("X"))
    print(repr(memory.getvalue()))
print(memory.closed)
memory.flush()
memory.close()

endings: list[str | None] = [None, "", "\n", "\r", "\r\n"]
for newline in endings:
    data = io.StringIO("λ\r\n中\rB\n", newline)
    show_lines(data)
    print(data.seek(0, 2))
    print(data.write("\r"))
    print(data.write("\n"))
    print(repr(data.getvalue()))
    data.close()

ordered = io.StringIO(newline=newline_text(), initial_value=initial_text())
print(repr(ordered.getvalue()))
empty = io.StringIO(None)
print(repr(empty.getvalue()))
print(empty.seek(5))
print(empty.truncate(None))
print(empty.tell())
print(repr(empty.getvalue()))
print(empty.write("🙂"))
print(repr(empty.getvalue()))

for mode in [0, 1, 2, 12]:
    try:
        print(empty.seek(-3, mode))
    except (ValueError, OSError) as error:
        print(str(error))
try:
    empty.truncate(-3)
except ValueError as error:
    print(str(error))
try:
    io.StringIO("", "bad")
except ValueError as error:
    print(str(error))
try:
    empty.fileno()
except OSError as error:
    print(str(error))
empty.close()
try:
    empty.getvalue()
except ValueError as error:
    print(str(error))
try:
    empty.tell()
except ValueError as error:
    print(str(error))
try:
    empty.write("")
except ValueError as error:
    print(str(error))
try:
    empty.read(1 << 100)
except OverflowError as error:
    print(str(error))
try:
    empty.seek(1 << 100)
except OverflowError as error:
    print(str(error))
try:
    with empty:
        print("closed stream entered")
except ValueError as error:
    print(str(error))
