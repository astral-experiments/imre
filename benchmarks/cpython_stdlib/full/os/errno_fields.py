"""Check projections and display of real native OS failures; not an upstream test ID."""
import os


def inspect(error: Exception) -> None:
    if not isinstance(error, OSError):
        raise AssertionError("expected OSError")
    # A syscall populates errno and strerror; an OSError constructed with a
    # single message leaves both absent.
    code = error.errno
    if code is not None:
        print(code)
    else:
        print("no errno")
    reason = error.strerror
    if reason is not None:
        print(reason)
    else:
        print("no strerror")
    name = error.filename
    if isinstance(name, bytes):
        print(repr(name))
    elif isinstance(name, str):
        print(repr(name))
    else:
        print("no filename")
    print(error.filename2 is None)
    print(str(error))
    print(f"{error!r}")


try:
    os.open(b"missing-\xff", os.O_RDONLY)
except FileNotFoundError as error:
    inspect(error)
try:
    os.open("missing-name", os.O_RDONLY)
except FileNotFoundError as error:
    inspect(error)
path = "missing-" + "built-in-open"
try:
    open(path)
except FileNotFoundError as error:
    name = error.filename
    if not isinstance(name, str):
        raise AssertionError("expected string filename")
    print(name is path)
try:
    os.close(-1)
except OSError as error:
    inspect(error)
try:
    raise OSError("manual")
except OSError as error:
    inspect(error)
