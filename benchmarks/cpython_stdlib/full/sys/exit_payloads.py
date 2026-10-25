"""Supplemental process fixture for SystemExit payload normalization and ownership."""
import sys


def record(value: int) -> int:
    print(value)
    return value


def empty() -> tuple[()]:
    print("empty called")
    return ()


def pair(value: int) -> tuple[int, int]:
    return value, value + 1


def run(mode: str) -> None:
    if mode == "empty-call":
        sys.exit(empty())
    elif mode == "singleton":
        sys.exit((42,))
    elif mode == "singleton-none":
        sys.exit((None,))
    elif mode == "tuple":
        sys.exit(pair(17))
    elif mode == "nested":
        sys.exit((("héllo", (1, 2)),))
    elif mode == "constructor":
        raise SystemExit((42,))
    elif mode == "constructor-empty":
        raise SystemExit(())
    elif mode == "constructor-pair":
        raise SystemExit(17, 23)
    elif mode == "extra":
        sys.exit(record(1), record(2))  # ty: ignore[too-many-positional-arguments]
    elif mode == "keyword":
        sys.exit(status=record(3))  # ty: ignore[positional-only-parameter-as-kwarg]


if sys.argv[1] == "catch":
    try:
        run(sys.argv[2])
    except SystemExit as error:
        print(repr(error.code))
        print(str(error))
        print(isinstance(error.code, tuple))
        print(bool(error.code))
        print(error.code == (17, 23))
    except TypeError as error:
        print(str(error))
else:
    run(sys.argv[2])
