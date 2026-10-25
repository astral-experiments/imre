import sys
from sys import exception

def current() -> BaseException | None:
    return exception()

def inner(outer: BaseException) -> None:
    assert current() is outer
    try:
        raise ValueError("inner")
    except ValueError as caught:
        assert sys.exception() is caught
        assert current() is not outer
    assert current() is outer

def main() -> None:
    assert sys.exception() is None
    try:
        raise RuntimeError("outer")
    except RuntimeError as caught:
        inner(caught)
        assert current() is caught
        saved = current()
        assert saved is caught
        print(str(caught))
    assert current() is None
    print("restored")

main()
