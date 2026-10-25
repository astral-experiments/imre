import json
import sys
import unittest
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
