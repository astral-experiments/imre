"""A live stream, its aliases and native TextIO consumers must agree with CPython."""
from io import StringIO
from typing import TextIO
import csv
import json


def write_row(file: TextIO) -> None:
    file.write("f1,f2,f3\r\n")
    file.write("1,abc,f\r\n")


def inspect_mode(newline: str | None) -> None:
    s = StringIO(initial_value="a\r\nb\rc\n雪🙂", newline=newline)
    print(repr(s.getvalue()))
    print(s.tell())
    print(repr(s.readline(2)))
    print(repr(s.readline(1)))
    for line in s:
        print(repr(line))
    print(repr(s.read()))
    print(repr(s.read(None)))
    s.seek(0)
    print(repr(s.readlines(3)))
    print(s.tell())
    s.seek(0, 2)
    print(s.write("x\r"))
    print(s.write("\ny"))
    print(repr(s.getvalue()))


def run() -> None:
    shared = StringIO('"one\r\ntwo",x\r\nskip,y\nlast,z\r', newline="")
    rows = csv.reader(shared)
    print(repr(next(rows)))
    print(rows.line_num)
    print(repr(shared.readline()))
    print(repr(next(rows)))
    shared.seek(0)
    print(repr(next(rows)))
    print(rows.line_num)
    shared.close()
    try:
        csv.reader(shared)
    except ValueError as exc:
        print(str(exc))
    sink = StringIO()
    json.dump(["streaming API"], sink)
    print(sink.getvalue())
    source = StringIO('["streaming API"]')
    print(json.load(source)[0] == "streaming API")
    inspect_mode(None)
    inspect_mode("")
    inspect_mode("\n")
    inspect_mode("\r")
    inspect_mode("\r\n")
    lines = StringIO("a\r\nb\nc", newline="")
    stream_iter = iter(lines)
    print(stream_iter is lines)
    print(StringIO() is not lines)
    print(repr(next(stream_iter)))
    print(repr(lines.__next__()))
    print(repr(next(lines)))
    print(repr(next(stream_iter, "end")))
    lines.seek(0)
    print(repr(next(stream_iter)))
    lines.seek(0, 2)
    lines.writelines(("d", "\n"))
    lines.writelines(["e", "\r"])
    print(repr(lines.getvalue()))
    print(repr(lines.newlines))
    print(lines.encoding is None)
    print(lines.errors is None)
    print(lines.line_buffering)
    lines.close()
    print(lines.encoding is None)
    print(lines.errors is None)
    try:
        iter(lines)
    except ValueError as exc:
        print(str(exc))
    try:
        print(next(lines, "end"))
    except ValueError as exc:
        print(str(exc))
    try:
        print(lines.readlines())
    except ValueError as exc:
        print(str(exc))
    history = StringIO(newline=None)
    print(repr(history.newlines))
    history.write("a\r")
    print(repr(history.newlines))
    history.write("b\r\n")
    saved = history.newlines
    print(repr(saved))
    history.write("\n")
    history.truncate(0)
    print(repr(saved))
    print(repr(history.newlines))
    print(repr(StringIO("a\rb\n", newline="\0tail").readline()))
    prefix = StringIO()
    try:
        prefix.writelines(("keep", 1, "unreached"))
    except TypeError as exc:
        print(str(exc))
    print(prefix.getvalue())
    prefix.close()
    try:
        prefix.write(1)
    except TypeError as exc:
        print(str(exc))
    try:
        prefix.writelines(("closed", 1))
    except ValueError as exc:
        print(str(exc))
    try:
        prefix.seek(1 << 100)
    except OverflowError as exc:
        print(str(exc))
    f = StringIO()
    write_row(f)
    print(repr(f.getvalue()))
    f.seek(0)
    print(repr(f.readline()))
    print(repr(f.readline()))
    print(repr(f.readline()))
    f.seek(1)
    print(f.write("雪🙂"))
    print(f.tell())
    print(f.truncate(3))
    f.seek(6)
    print(f.write(""))
    print(repr(f.getvalue()))
    print(f.write("z"))
    print(repr(f.getvalue()))
    print(f.truncate(90))
    print(f.tell())
    print(repr(f.getvalue()))
    for offset in (0, 1, -1):
        try:
            print(f.seek(offset, 1))
        except OSError as exc:
            print(str(exc))
    with StringIO(None) as g:
        alias = g
        print(g.readable())
        print(g.writable())
        print(g.seekable())
        print(g.isatty())
        print(g.closed)
    print(alias.closed)
    alias.flush()
    try:
        alias.isatty()
    except ValueError as exc:
        print(str(exc))
    try:
        alias.readable()
    except ValueError as exc:
        print(str(exc))
    try:
        alias.getvalue()
    except ValueError as exc:
        print(str(exc))
    try:
        StringIO(newline="bad")
    except ValueError as exc:
        print(str(exc))


run()
