import json
import sys
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from tempfile import TemporaryDirectory


def arguments(exc: BaseException) -> object:
    return exc.args


def show(exc: BaseException) -> None:
    args = exc.args
    print(repr(exc))
    print(repr(args))
    print(len(args))
    print(isinstance(args, tuple))
    if len(args):
        print(repr(args[0]))
        print(repr(args[-1]))
    try:
        args[20]
    except IndexError as error:
        print(str(error))


class Marker(ValueError):
    pass


class Reordered(ValueError):
    def __init__(self, code: int, text: str) -> None:
        self.code = code
        super().__init__(text, code)


@dataclass
class Hidden:
    shown: int
    hidden: int = field(repr=False)


def escape(items: list[str]) -> object:
    try:
        raise ValueError(items)
    except ValueError as error:
        return arguments(error)

def identities(exc: BaseException) -> None:
    saved = exc.args
    print(saved is exc.args)
    print(exc.args is not saved)
    empty = ()
    print(empty is exc.args)

case = unittest.TestCase()
with case.assertRaises((TypeError, Marker)) as caught:
    raise Marker("We expect")
print(caught.exception.args[0] == "We expect")
show(caught.exception)
try:
    raise Reordered(7, "reordered")
except Exception as caught_base:
    show(caught_base)
show(ValueError())
show(ValueError(""))
show(TypeError(None))
show(ValueError(2**83))
show(ValueError(1.25))
show(KeyError("missing"))
show(ImportError("one", "two", name="third"))
show(SystemExit())
show(SystemExit(None))
show(SystemExit((1, 2)))
show(SystemExit(1, 2))
try:
    sys.exit((1, 2))
except BaseException as error:
    show(error)
try:
    b"ab\xff".decode()
except UnicodeDecodeError as error:
    show(error)
try:
    b"ok\xfa".decode("ascii")
except UnicodeDecodeError as error:
    show(error)
try:
    json.loads("{")
except json.JSONDecodeError as error:
    show(error)
for raw in [
    b"\xff\xfe{\x00x", b"\xfe\xff\x00{x", b"\x00\x00\xfe\xff\x00\x00\x00{x",
    b"\xff\xfe\x00\x00\x00\x00\x11\x00", b"\xef\xbb\xbf\xff",
]:
    try:
        json.loads(raw)
    except UnicodeDecodeError as error:
        show(error)
try:
    open("nonexistent-exception-args-probe", "rb")
except OSError as error:
    show(error)

with TemporaryDirectory() as directory:
    target = Path(directory) / "input"
    target.write_bytes(b"a\x80b")
    try:
        target.read_text(encoding="ASCII")
    except UnicodeDecodeError as error:
        show(error)
    with target.open("r") as handle:
        try:
            handle.read()
        except UnicodeDecodeError as error:
            show(error)
    for suffix, rest in [(b"\r", b"\xffB"), (b"\xf0", b"\xffQ"), (b"\r\xf0", b"\xffB")]:
        count = 8192 - len(suffix)
        target.write_bytes(b"A" * (count - 1) + b"\n" + suffix + rest)
        with target.open("r") as handle:
            print(len(handle.readline()))
            try:
                handle.readline()
            except UnicodeDecodeError as error:
                show(error)
    for encoding in ["ascii", "latin-1"]:
        try:
            target.write_text("a\n\u0141\u0142", encoding=encoding, newline="\r\n")
        except UnicodeEncodeError as error:
            show(error)


items = ["old"]
saved = escape(items)
items.append("new")
print(repr(saved))
assert isinstance(saved, tuple)
print(repr(saved[0]))
print(len(saved[0]))
print(repr(saved[0][-1]))
print(saved[0] is items)

numbers = [4]
try:
    raise ValueError(numbers)
except Exception as error:
    args = error.args
    numbers.append(2**90)
    print(repr(args))
    print(str(error))
    print(args[0] is numbers)

for bad in [ValueError(1.25), ValueError(True)]:
    scalar = bad.args[0]
    try:
        len(scalar)
    except TypeError as error:
        print(str(error))
    try:
        scalar[0]
    except TypeError as error:
        print(str(error))

mapping = {3: "value"}
mapped = ValueError(mapping).args[0]
print(isinstance(mapped, dict))
print(isinstance(mapped, list))
print(bool(mapped))
print(len(mapped))
print(repr(mapped[3]))
print(mapped == mapping)
mapping.clear()
print(len(mapped))
print(bool(mapped))
print(mapped == mapping)
try:
    mapped[3]
except KeyError as error:
    print(repr(error.args))
try:
    ValueError({"3": "different key"}).args[0][3]
except KeyError as error:
    print(repr(error.args))

members = {3}
held = ValueError(members).args[0]
print(isinstance(held, set))
print(isinstance(held, list))
print(len(held))
print(bool(held))
members.discard(3)
print(len(held))
print(bool(held))
try:
    held[0]
except TypeError as error:
    print(str(error))

limit = 10 ** 4300
for nested in [
    ValueError({1: limit}), ValueError({limit: "key"}), ValueError({-limit}),
    ValueError([("n", limit)]), ValueError((limit,)),
]:
    try:
        repr(nested.args)
    except ValueError as error:
        print(str(error))
print(len(repr(ValueError({1: limit - 1}).args)))
print(repr(ValueError((Hidden(4, limit),)).args))

imp = ImportError("old")
prior = arguments(imp)
imp.__init__("new", 2)
print(repr(prior))
print(repr(arguments(imp)))
print(prior is not imp.args)
identities(ValueError())
identities(ValueError("identical contents"))
print(ValueError("same").args is ValueError("same").args)
status = SystemExit(1, 2)
print(status.args is status.code)
status = SystemExit((1, 2))
print(status.args is status.code)
