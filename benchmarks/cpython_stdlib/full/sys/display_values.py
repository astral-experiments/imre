"""Native process fixture for the builtins binding used by sys.__displayhook__."""

import builtins as namespace
import errno
import sys


def main() -> None:
    print(hasattr(namespace, "_"))
    try:
        print(namespace._)
    except AttributeError as error:
        print(str(error))
    try:
        del namespace._
    except AttributeError as error:
        print(str(error))
    sys.__displayhook__(None)
    print(hasattr(namespace, "_"))
    sys.__displayhook__(42)
    print(namespace._ == 42)
    previous = namespace._
    namespace._ = "a'b"
    print(namespace._)
    print(repr(namespace._))
    sys.__displayhook__(None)
    print(previous == 42)
    print(namespace._ == "a'b")
    numbers = [1, 2]
    sys.__displayhook__(numbers)
    captured = namespace._
    numbers.append(3)
    print(captured)
    print(isinstance(captured, list))
    print(bool(captured))
    print(len(captured))
    print(captured[-1] == 3)
    print(captured == [1, 2, 3])
    namespace._ = previous
    del namespace._
    print(captured)
    print(hasattr(namespace, "_"))
    namespace._ = True
    print(1 == namespace._)
    print(namespace._ == True)
    print(isinstance(namespace._, int))
    namespace._ = None
    print(namespace._ is None)
    print(bool(namespace._))
    print(hasattr(namespace, "_"))
    sys.__displayhook__(b"bytes")
    print(repr(namespace._))
    try:
        raise SystemExit(("argv", 7))
    except SystemExit as error:
        namespace._ = error.code
    print(namespace._ == ("argv", 7))
    print(repr(namespace._))
    try:
        sys.__displayhook__()  # ty: ignore[missing-argument]
    except TypeError as error:
        print(str(error))

    key = "errorcode"
    namespace._ = getattr(errno, key)
    print(isinstance(namespace._, dict))
    print(len(namespace._) == len(errno.errorcode))
    print(namespace._ == errno.errorcode)
    key = "__loader__"
    sys.__displayhook__(getattr(errno, key))
    print(repr(namespace._))
    key = "EDOM"
    sys.__displayhook__(getattr(errno, key))
    print(namespace._ == errno.EDOM)


main()
