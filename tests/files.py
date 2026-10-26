"""Compare sealed UTF-8 files, context cleanup, and process arguments with CPython."""

import argparse
import errno
import gzip
import os
import resource
import selectors
import socket
import subprocess
import sys
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path
from textwrap import dedent

ACCEPTED = {
    "string_io_import": "from io import StringIO\nStringIO()",
    "concat_write_order": r"""
from typing import TextIO

def close_in_argument(stream: TextIO) -> str:
    print("closing argument")
    stream.close()
    return "closed"

def fail() -> str:
    print("failing argument")
    raise ValueError("argument failure")

def remove_owner(streams: list[TextIO]) -> str:
    streams.pop()
    return "only receiver remains"

def write_and_return(stream: TextIO) -> int:
    try:
        stream.write("return" + ":body\n")
        return 7
    finally:
        stream.write("return" + ":finally\n")

with open("output.txt", "w", encoding="utf-8") as stream:
    stream.write("prefix" + "λ\0🙂" + "\n")
    print(stream.write("λ" + "🙂\0"))
    print(stream.write("" + ""))
    print(write_and_return(stream))
    try:
        stream.write("must not write" + close_in_argument(stream) + "tail")
    except ValueError:
        print("closed after arguments")

try:
    with open("late.txt", "w", encoding="utf-8") as stream:
        stream.write("must not write" + fail() + "tail")
except ValueError:
    print("no partial expression write")

reader = open("input.txt", "r", encoding="utf-8")
try:
    reader.write("" + "")
except OSError:
    print("empty concat still checks write mode")
reader.close()
try:
    reader.write("" + "")
except ValueError:
    print("empty concat still checks closed state")
try:
    reader.write(fail() + "must not evaluate the closed file")
except ValueError as error:
    print(str(error))

streams: list[TextIO] = [open("rooted.txt", "w", encoding="utf-8")]
streams[0].write(
    "root:"
    + str(1000000000000000000000000000000000000000000000000000000000009)
    + remove_owner(streams)
    + ":end"
)
print(len(streams))

text = "large write"
for index in range(12):
    text = text + text
with open("large.txt", "w", encoding="utf-8") as stream:
    stream.write(text + "\0λ" + text)
try:
    with open("failing-output", "w", encoding="utf-8") as stream:
        stream.write("prefix" + text + "suffix")
except OSError:
    print("discarded write failed")
""",
    "text_roundtrip": r"""
with open("input.txt", "r", encoding="utf-8") as source:
    print(source.closed)
    text = source.read()
    print(text)
    print(len(text))
    print(source.read())
    source.flush()
print(source.closed)
with open("output.txt", "w", encoding="utf-8") as output:
    print(output.write(text))
    print(output.write("λ🙂\0tail"))
    print(output.write(""))
    output.flush()
    print(output.closed)
print(output.closed)
with open("empty.txt", "r", encoding="utf-8") as empty:
    print(empty.read())
    print(empty.read())
with open("buffered.txt", "w", encoding="utf-8") as writer:
    for size in [122880, 8191, 2]:
        writer.write(bytes(size).decode())
        with open("buffered.txt", "rb") as reader:
            print(len(reader.read()))
    writer.flush()
    with open("buffered.txt", "rb") as reader:
        print(len(reader.read()))
""",
    "splitlines_argument": 'print("a\\nb".splitlines(True))',
    "splitlines_and_formatting": r"""
def nothing() -> None:
    print("None argument evaluated")

texts = ["", "\n", "\r\n", "a\r\nb\rc\nd\ve\ff\x1cg\x1dh\x1ei\x85j\u2028k\u2029", "a\x1fb", "x\n\n", "\0é🙂"]
for text in texts:
    lines = text.splitlines()
    print(len(lines))
    for line in lines:
        print(line)
print(str())
print(str(0))
print(str(-17))
print(str(999999999999999999999999999999999999999999))
print(str(True))
print(str(False))
print(str(None))
print(str("é\0🙂"))
print(str(nothing()))
print(str(ValueError("existing exception")))
""",
    "stored_aliases": r"""
from typing import TextIO

class Holder:
    stream: TextIO

    def __init__(self, stream: TextIO) -> None:
        self.stream = stream

def opened() -> TextIO:
    return open("input.txt", "r", encoding="utf-8")

stream = opened()
holder = Holder(stream)
sequence: list[TextIO] = [stream]
mapping: dict[str, TextIO] = {"source": stream}
optional: TextIO | None = mapping.get("source")
if optional is not None:
    print(optional.read())
holder.stream.close()
print(stream.closed)
print(sequence[0].closed)
print(mapping["source"].closed)
stream.close()
print(stream.closed)
""",
    "target_rebinding": r"""
original = open("input.txt", "r", encoding="utf-8")
with original as target:
    target = open("output.txt", "w", encoding="utf-8")
    target.write("replacement remains open")
print(original.closed)
print(target.closed)
target.close()
print(target.closed)
""",
    "multiple_managers": r"""
first = open("input.txt", "r", encoding="utf-8")
try:
    with first as left, open("missing", "r", encoding="utf-8") as right:
        print("unreachable")
except FileNotFoundError:
    print(first.closed)

first = open("input.txt", "r", encoding="utf-8")
second = open("empty.txt", "r", encoding="utf-8")
second.close()
try:
    with first as left, second as right:
        print("unreachable")
except ValueError:
    print(first.closed)
    print(second.closed)

with open("input.txt", "r", encoding="utf-8") as one, open("output.txt", "w", encoding="utf-8") as two:
    two.write(one.read())
print(one.closed)
print(two.closed)
""",
    "returns_and_loops": r"""
from typing import TextIO

def read_return() -> str:
    with open("input.txt", "r", encoding="utf-8") as stream:
        return stream.read()

def closed_return() -> TextIO:
    with open("input.txt", "r", encoding="utf-8") as stream:
        return stream

print(read_return())
print(closed_return().closed)
streams: list[TextIO] = []
for index in range(4):
    stream = open("input.txt", "r", encoding="utf-8")
    streams.append(stream)
    with stream:
        if index == 0:
            continue
        if index == 2:
            break
        print(stream.read())
for stream in streams:
    print(stream.closed)
stream = open("empty.txt", "r", encoding="utf-8")
stream.close()
index = 0
while index < 3:
    stream = open("empty.txt", "r", encoding="utf-8")
    index += 1
    with stream:
        if index == 1:
            continue
        break
print(stream.closed)
""",
    "body_exception_and_active_error": r"""
def again() -> None:
    raise

stream = open("output.txt", "w", encoding="utf-8")
try:
    with stream:
        stream.write("flushed on exceptional exit")
        raise ValueError("body")
except ValueError as problem:
    print(problem)
    print(stream.closed)
    with open("empty.txt", "r", encoding="utf-8") as inner:
        try:
            again()
        except ValueError as active:
            print(active)
    print(inner.closed)
""",
    "closed_enter_and_binding": r"""
closed = open("empty.txt", "r", encoding="utf-8")
closed.close()
target = open("output.txt", "w", encoding="utf-8")
try:
    with closed as target:
        print("unreachable")
except ValueError:
    print(target.closed)
target.write("unchanged target")
target.close()
with open("empty.txt", "r", encoding="utf-8") as stream:
    stream.close()
    print(stream.closed)
print(stream.closed)
""",
    "mode_and_closed_errors": r"""
from io import UnsupportedOperation as Unsupported

reader = open("input.txt", "r", encoding="utf-8")
writer = open("output.txt", "w", encoding="utf-8")
try:
    reader.write("bad")
except Unsupported:
    print("write unsupported")
try:
    reader.write("bad")
except OSError:
    print("also OSError")
try:
    writer.read()
except ValueError:
    print("also ValueError")
reader.close()
writer.close()
try:
    reader.write("bad")
except Unsupported:
    print("wrong precedence")
except ValueError:
    print("closed write")
try:
    writer.read()
except Unsupported:
    print("wrong precedence")
except ValueError:
    print("closed read")
try:
    reader.flush()
except ValueError:
    print("closed flush")
reader.close()
writer.close()
""",
    "path_errors": r"""
try:
    open("missing", "r", encoding="utf-8")
except FileNotFoundError:
    print("missing")
try:
    open("directory", "r", encoding="utf-8")
except IsADirectoryError:
    print("directory read")
try:
    open("directory", "w", encoding="utf-8")
except OSError:
    print("directory write")
try:
    open("input.txt/child", "r", encoding="utf-8")
except NotADirectoryError:
    print("not directory")
try:
    open("invalid\0path", "r", encoding="utf-8")
except ValueError:
    print("NUL path")
with open("space é🙂.txt", "w", encoding="utf-8") as output:
    output.write("unicode filename")
""",
    "decode_errors": r"""
for path in ["invalid.txt", "truncated.txt"]:
    try:
        with open(path, "r", encoding="utf-8") as stream:
            stream.read()
    except UnicodeDecodeError:
        print("decode")
    try:
        with open(path, "r", encoding="utf-8") as stream:
            stream.read()
    except UnicodeError:
        print("unicode")
    try:
        with open(path, "r", encoding="utf-8") as stream:
            stream.read()
    except ValueError:
        print("value")
""",
    "implicit_close": r"""
def write_file() -> None:
    stream = open("output.txt", "w", encoding="utf-8")
    stream.write("last owner closes and flushes")

write_file()
with open("output.txt", "r", encoding="utf-8") as stream:
    print(stream.read())
""",
    "nested_record_release_order": r"""
from dataclasses import dataclass
from typing import BinaryIO

@dataclass(repr=False, eq=False)
class Leaf:
    stream: BinaryIO

@dataclass(repr=False, eq=False)
class Group:
    first: Leaf
    direct: BinaryIO
    nested: list[Leaf]
    mapped: dict[tuple[int, int], Leaf]
    pair: tuple[Leaf, Leaf]
    last: Leaf

def writer(mark: bytes) -> BinaryIO:
    stream = open("order.bin", "ab")
    stream.write(mark)
    return stream

def group(shared: bool) -> Group:
    first = Leaf(writer(b"A"))
    last = first if shared else Leaf(writer(b"D"))
    nested = [Leaf(writer(b"C")), Leaf(writer(b"E")), Leaf(writer(b"H")), Leaf(writer(b"I"))]
    mapped: dict[tuple[int, int], Leaf] = {}
    for i, mark in enumerate([b"J", b"K", b"L", b"M"]):
        mapped[(i, i + 1)] = Leaf(writer(mark))
    return Group(first, writer(b"B"), nested, mapped,
                 (Leaf(writer(b"F")), Leaf(writer(b"G"))), last)

def discard(shared: bool) -> None:
    owner = group(shared)
    print(owner.first.stream.closed)

with open("order.bin", "wb") as empty:
    pass
discard(False)
discard(True)
with open("order.bin", "rb") as result:
    print(result.read())
""",
    "list_release_order": r"""
from typing import BinaryIO

def writer(mark: bytes) -> BinaryIO:
    stream = open("order.bin", "ab")
    stream.write(mark)
    return stream

def discard(shared: bool) -> None:
    streams: list[BinaryIO] = [writer(b"A"), writer(b"B")]
    if shared:
        streams.append(streams[0])
    else:
        streams.append(writer(b"C"))
    streams.append(writer(b"D"))
    print(len(streams))

with open("order.bin", "wb") as empty:
    pass
discard(False)
discard(True)
with open("order.bin", "rb") as result:
    print(result.read())
""",
    "projected_tuple_release_order": r"""
from dataclasses import dataclass
from typing import BinaryIO

@dataclass(repr=False, eq=False)
class Leaf:
    stream: BinaryIO

def writer(mark: bytes) -> BinaryIO:
    stream = open("order.bin", "ab")
    stream.write(mark)
    return stream

def projected() -> None:
    streams = (writer(b"A"), writer(b"B"), writer(b"C"))
    print(streams[1].closed)

def nested() -> None:
    leaves = (Leaf(writer(b"D")), Leaf(writer(b"E")), Leaf(writer(b"F")))
    print(leaves[1].stream.closed)

def replaced(count: int) -> None:
    streams = (writer(b"G"), writer(b"H"))
    for i in range(count):
        streams = (writer(b"I"), writer(b"J"))
        print(streams[0].closed)

def fail() -> BinaryIO:
    raise ValueError("stop")

def interrupted() -> None:
    streams = (writer(b"K"), writer(b"L"))
    try:
        streams = (writer(b"M"), fail())
    except ValueError:
        print(streams[0].closed)

with open("order.bin", "wb") as empty:
    pass
projected()
nested()
replaced(2)
interrupted()
with open("order.bin", "rb") as result:
    print(result.read())
""",
    "flush_and_close_failure": r"""
stream = open("failing-output", "w", encoding="utf-8")
stream.write("buffered")
try:
    stream.flush()
except OSError:
    print("flush failed")
print(stream.closed)
try:
    stream.close()
except OSError:
    print("close failed")
print(stream.closed)
stream.close()
print(stream.closed)
""",
    "close_replaces_completion": r"""
def owned_return() -> list[int]:
    with open("failing-output", "w", encoding="utf-8") as stream:
        stream.write("buffered")
        return [999999999999999999999999999999999999]

outer = open("output.txt", "w", encoding="utf-8")
inner = open("failing-output", "w", encoding="utf-8")
try:
    with outer:
        outer.write("outer still closes")
        with inner:
            inner.write("buffered")
            raise ValueError("body displaced")
except OSError:
    print("close replaces body")
print(outer.closed)
print(inner.closed)
try:
    owned_return()
except OSError:
    print("close replaces return")
""",
    "descriptor_recovery": r"""
for index in range(600):
    try:
        with open("empty.txt", "r", encoding="utf-8") as first, open("missing", "r", encoding="utf-8") as second:
            pass
    except FileNotFoundError:
        pass
with open("output.txt", "w", encoding="utf-8") as output:
    output.write("no descriptor exhaustion")
print("finished")
""",
    "argument_aliases": r"""
import sys as process
from bridge import arguments as args
from bridge import opened

print(len(args))
for index in range(1, len(process.argv)):
    print(args[index])
args[0] = "shared program identity"
print(process.argv[0])
process.argv.append("added")
print(args.pop())
args[1] = "replaced empty argument"
print(process.argv[1])
with opened() as stream:
    print(stream.read())
""",
}

ERRORS = {
    "uncaught_missing": ('open("missing", "r", encoding="utf-8")', "FileNotFoundError"),
    "uncaught_decode": (
        'with open("invalid.txt", "r", encoding="utf-8") as stream:\n    stream.read()',
        "UnicodeDecodeError",
    ),
    "uncaught_close": (
        (
            'with open("failing-output", "w", encoding="utf-8") as stream:\n'
            '    stream.write("buffered")\n    raise ValueError("replaced")'
        ),
        "OSError",
    ),
}

REJECTED = {
    "dynamic_mode": 'mode = "r"\nopen("input.txt", mode, encoding="utf-8")',
    "binary_mode": 'open("input.txt", "rb", encoding="utf-8")',
    "append_mode": 'open("input.txt", "a", encoding="utf-8")',
    "encoding": 'open("input.txt", "r", encoding="latin1")',
    "dynamic_encoding": 'encoding = "utf-8"\nopen("input.txt", "r", encoding=encoding)',
    "unsupported_newline": 'open("input.txt", "r", encoding="utf-8", newline="other")',
    "read_size": 'with open("input.txt", "r", encoding="utf-8") as stream:\n    stream.read(1)',
    "readline_size": 'with open("input.txt", "r", encoding="utf-8") as stream:\n    stream.readline(2)',
    "gzip_write": 'import gzip\ngzip.open("output.gz", "wb")',
    "gzip_text_codec": 'import gzip\ngzip.open("input.gz", "rt", encoding="latin1")',
    "gzip_binary_encoding": 'import gzip\ngzip.open("input.gz", "rb", encoding="utf-8")',
    "gzip_text_no_encoding": 'import gzip\ngzip.open("input.gz", "rt")',
    "gzip_dynamic_mode": 'import gzip\nmode = "rt"\ngzip.open("input.gz", mode, encoding="utf-8")',
    "seek": 'with open("input.txt", "r", encoding="utf-8") as stream:\n    stream.seek(0)',
    "write_type": 'with open("output.txt", "w", encoding="utf-8") as stream:\n    stream.write(1)',
    "closed_assignment": 'stream = open("input.txt", "r", encoding="utf-8")\nstream.closed = False',
    "str_resource_list": "print(str([open('input.txt', 'r')]))",
    "text_constructor": "from typing import TextIO\nTextIO()",
    "decode_constructor": 'raise UnicodeDecodeError("not a valid constructor")',
    "generic_context": "with [1] as values:\n    print(values[0])",
    "tuple_target": 'with open("input.txt", "r", encoding="utf-8") as (left, right):\n    pass',
    "attribute_target": 'from typing import TextIO\nclass Box:\n    stream: TextIO\n    def __init__(self, stream: TextIO) -> None:\n        self.stream = stream\nbox = Box(open("input.txt", "r", encoding="utf-8"))\nwith box.stream as box.stream:\n    pass',
    "io_unknown_member": "from io import TextCapture\nTextCapture()",
    "string_io_binary_input": "from io import StringIO\nStringIO(b'input')",
    "typing_member": "from typing import Any\nvalue: Any = 1",
    "module_rebind": 'import sys\nsys.argv = ["replacement"]',
    "alias_rebind": 'from sys import argv\nargv = ["replacement"]',
    "parameter_shadow": "import sys\ndef f(sys: int) -> int:\n    return sys",
    "optional_access": "from typing import TextIO\nstream: TextIO | None = None\nstream.read()",
    "with_uninitialized": 'if False:\n    with open("input.txt", "r", encoding="utf-8") as stream:\n        pass\nprint(stream.closed)',
}

BRIDGE = """from sys import argv as arguments
from typing import TextIO as Stream

def opened() -> Stream:
    return open("input.txt", "r", encoding="utf-8")
"""

INVALID_ARGUMENT = """import sys
for turn in range(2):
    try:
        print(len(sys.argv))
    except UnicodeDecodeError:
        print("invalid UTF-8 argument")
"""


BROKEN_PIPE = """stream = open("fifo", "w", encoding="utf-8")
print("ready")
input()
try:
    stream.write("buffered")
    stream.flush()
except OSError:
    print("write failed")
try:
    stream.close()
except OSError:
    print("close failed")
print(stream.closed)
"""


def write_non_utf8_filename(path: bytes) -> bool:
    try:
        with open(path, "wb") as stream:
            stream.write(b"native filename")
    except OSError as error:
        if error.errno != errno.EILSEQ:
            raise
        return False  # APFS requires UTF-8 filenames.
    return True


def environment() -> dict[str, str]:
    return {**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"}


def descriptor_limit() -> None:
    """Expose leaked handles without depending on the host's generous default limit."""
    _, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    soft = 64 if hard == resource.RLIM_INFINITY else min(64, hard)
    resource.setrlimit(resource.RLIMIT_NOFILE, (soft, hard))


def run_with_failing_output(command, *, cwd, **options):
    with ExitStack() as stack:
        target = "/dev/full"
        if sys.platform == "darwin":
            # Reopening the socket succeeds, but writes fail with EDESTADDRREQ.
            sock = stack.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM))
            target = f"/dev/fd/{sock.fileno()}"
            options["pass_fds"] = (sock.fileno(),)
        path = Path(cwd) / "failing-output"
        path.symlink_to(target)
        stack.callback(path.unlink)
        return subprocess.run(command, cwd=cwd, **options)


def run(command, cwd: Path, *, limit_descriptors: bool = False, runner=subprocess.run):
    return runner(
        command,
        cwd=cwd,
        input=b"",
        capture_output=True,
        timeout=60,
        check=False,
        env=environment(),
        preexec_fn=descriptor_limit if limit_descriptors else None,
    )


def seed(directory: Path, case: str) -> None:
    directory.mkdir()
    (directory / "input.txt").write_bytes("alpha\r\néλ🙂\rzero\0tail\n".encode())
    (directory / "empty.txt").write_bytes(b"")
    (directory / "invalid.txt").write_bytes(b"valid prefix\xffinvalid")
    (directory / "truncated.txt").write_bytes(b"truncated \xf0\x9f")
    (directory / "output.txt").write_bytes(b"old data that must be truncated")
    (directory / "directory").mkdir()
    if case == "stream_reads":
        (directory / "truncated-after-open.gz").write_bytes(
            gzip.compress(b"x" * 300_000, compresslevel=0, mtime=0)
        )
        (directory / "long-rows.txt").write_bytes(
            b"a" * 32768 + b"\n" + b"b" * 32769 + b"\r\n" + b"c" * 160000 + b"\rdone"
        )
    if case == "stream_errors":
        first = gzip.compress(b"\n" * 65536, compresslevel=1, mtime=0)
        for name, length in [("partial", 131071), ("bulk", 262144)]:
            second = bytearray(gzip.compress(b"x" * length, compresslevel=6, mtime=0))
            second[10] &= ~1  # Clear BFINAL so the decoder encounters an invalid next block.
            (directory / f"{name}.gz").write_bytes(first + second)
    if case.startswith("stream_"):
        # Split both a CRLF and a four-byte UTF-8 scalar at 8 KiB boundaries.
        contents = b"a" * 8191 + b"\r\n" + b"b" * 8189 + "🙂\rC\n\nD\0".encode()
        (directory / "rows.txt").write_bytes(contents)
        members = gzip.compress(contents[:8192], mtime=0) + gzip.compress(contents[8192:], mtime=0)
        (directory / "rows.gz").write_bytes(members)
        (directory / "empty.gz").write_bytes(gzip.compress(b"", mtime=0))
        (directory / "padded.gz").write_bytes(members + b"\0" * 20)
        (directory / "short.gz").write_bytes(members[:-3])
        (directory / "crc.gz").write_bytes(members[:-8] + bytes([members[-8] ^ 1]) + members[-7:])
        (directory / "invalid-block.gz").write_bytes(bytes.fromhex("1f8b0800000000000003ff") + bytes(16))
        (directory / "invalid-text.gz").write_bytes(gzip.compress(b"bad\xff", mtime=0))
    if case == "stream_errors":
        header = bytes.fromhex("1f8b08040000000000ff") + b"\xff\xff" + bytes(65535)
        # A nonfinal stored block followed by a reserved block type.
        body = bytes.fromhex("00ffff0000") + b"x" * 65535 + b"\x07"
        (directory / "long-header.gz").write_bytes(header + body)


def snapshot(directory: Path) -> dict[str, bytes | None]:
    """Compare all filesystem effects, including untouched inputs and new paths."""
    return {
        str(path.relative_to(directory)): path.read_bytes() if path.is_file() else None
        for path in sorted(directory.rglob("*"))
    }


def check_result(result, category: str | None, label: str) -> None:
    if category:
        assert result.returncode == 1 and category.encode() + b":" in result.stderr, (
            label,
            result.returncode,
            result.stderr[:2000],
        )
    else:
        assert result.returncode == 0 and not result.stderr, (
            label,
            result.returncode,
            result.stderr[:2000],
        )
    assert b"Sanitizer" not in result.stderr, (label, result.stderr)


def broken_pipe(command: list[str], directory: Path) -> bytes:
    """Close the FIFO reader only after the child has opened its writer."""
    directory.mkdir()
    fifo = directory / "fifo"
    os.mkfifo(fifo)
    reader = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
    process = None
    try:
        process = subprocess.Popen(
            command,
            cwd=directory,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            env=environment(),
        )
        assert process.stdout is not None
        prefix = bytearray()
        deadline = time.monotonic() + 10
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while len(prefix) < len(b"ready\n"):
                remaining = deadline - time.monotonic()
                assert remaining > 0 and selector.select(remaining), (
                    "FIFO open handshake timed out"
                )
                part = os.read(process.stdout.fileno(), len(b"ready\n") - len(prefix))
                assert part, "child exited before FIFO open handshake"
                prefix.extend(part)
        assert prefix == b"ready\n", prefix
        os.close(reader)
        reader = -1
        output, error = process.communicate(input=b"continue\n", timeout=10)
        assert process.returncode == 0 and not error, (
            "broken FIFO must be a catchable error",
            process.returncode,
            error,
        )
        return bytes(prefix) + output
    finally:
        if reader >= 0:
            os.close(reader)
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=10)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    compiler = str(args.compiler.resolve())
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    passed = 0
    with tempfile.TemporaryDirectory(prefix="imre-files-") as directory:
        temporary = Path(directory)
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(dedent(source), encoding="utf-8")
            result = run([compiler, "check", str(path)], temporary)
            assert result.returncode > 0 and result.stderr, (name, result)
            passed += 1
        for module in ["sys", "typing", "io", "gzip", "zlib"]:
            project = temporary / f"shadow-{module}"
            project.mkdir()
            (project / "main.py").write_text(f"import {module}\n")
            (project / f"{module}.py").write_text("def value() -> int:\n    return 1\n")
            result = run([compiler, "check", str(project / "main.py")], temporary)
            assert result.returncode > 0 and b"standard-library" in result.stderr, (
                module,
                result.stderr,
            )
            passed += 1
        cases = [(name, source, None) for name, source in ACCEPTED.items()]
        for fixture in sorted((Path(__file__).parent / "fixtures/files").glob("*.py")):
            cases.append((fixture.stem, fixture.read_text(encoding="utf-8"), None))
        cases += [
            (name, source, category) for name, (source, category) in ERRORS.items()
        ]
        for name, source, category in cases:
            project = temporary / name
            project.mkdir()
            path = project / "main.py"
            path.write_text(dedent(source), encoding="utf-8")
            (project / "bridge.py").write_text(BRIDGE, encoding="utf-8")
            runner = run_with_failing_output if '"failing-output"' in source else subprocess.run
            argv = (
                ["", "é λ🙂", "space separated"] if name == "argument_aliases" else []
            )
            reference_dir = project / "cpython"
            seed(reference_dir, name)
            reference = run(
                [args.python, "-E", "-B", str(path), *argv],
                reference_dir,
                limit_descriptors=name == "descriptor_recovery",
                runner=runner,
            )
            check_result(reference, category, f"CPython {name}")
            expected_files = snapshot(reference_dir)
            for level in args.opt_levels:
                binary = project / f"native-O{level}"
                llvm = project / f"native-O{level}.ll"
                built = run(
                    [
                        compiler,
                        "build",
                        str(path),
                        "--module-root",
                        str(project),
                        "-o",
                        str(binary),
                        "--emit-llvm",
                        str(llvm),
                        "--opt-level",
                        level,
                        *flags,
                    ],
                    temporary,
                )
                assert built.returncode == 0, (name, level, built.stderr.decode())
                if name == "text_roundtrip":
                    assert "call void @imre_args_init" not in llvm.read_text()
                    assert "call void @imre_args_clear" not in llvm.read_text()
                native_dir = project / f"imre-O{level}"
                seed(native_dir, name)
                actual = run(
                    [str(binary), *argv],
                    native_dir,
                    limit_descriptors=name == "descriptor_recovery",
                    runner=runner,
                )
                check_result(actual, category, f"Imre {name} O{level}")
                assert actual.stdout == reference.stdout, (
                    name,
                    level,
                    actual.stdout,
                    reference.stdout,
                )
                assert snapshot(native_dir) == expected_files, (
                    name,
                    level,
                    "file bytes differ",
                )
                passed += 1

        # CPython surrogateescapes invalid Unix argv bytes; the native string domain
        # deliberately rejects them. Check retry and cleanup at that explicit boundary.
        path = temporary / "invalid_argument.py"
        path.write_text(INVALID_ARGUMENT)
        for level in args.opt_levels:
            binary = temporary / f"invalid-argument-O{level}"
            built = run(
                [
                    compiler,
                    "build",
                    str(path),
                    "-o",
                    str(binary),
                    "--opt-level",
                    level,
                    *flags,
                ],
                temporary,
            )
            assert built.returncode == 0, built.stderr.decode()
            actual = run([os.fsencode(binary), b"\xff"], temporary)
            check_result(actual, None, "invalid argument")
            assert actual.stdout == b"invalid UTF-8 argument\n" * 2, actual.stdout
            passed += 1
        path = temporary / "broken_pipe.py"
        path.write_text(BROKEN_PIPE)
        reference = broken_pipe(
            [args.python, "-I", str(path)], temporary / "pipe-cpython"
        )
        assert reference == b"ready\nwrite failed\nclose failed\nTrue\n", reference
        for level in args.opt_levels:
            binary = temporary / f"broken-pipe-O{level}"
            built = run(
                [
                    compiler,
                    "build",
                    str(path),
                    "-o",
                    str(binary),
                    "--opt-level",
                    level,
                    *flags,
                ],
                temporary,
            )
            assert built.returncode == 0, built.stderr.decode()
            actual = broken_pipe([str(binary)], temporary / f"pipe-imre-O{level}")
            assert actual == reference, (level, actual, reference)
            passed += 1
    print(
        f"Passed {passed} file/argument checks (optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
