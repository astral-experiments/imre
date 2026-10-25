"""Differential checks for the limit, real source frames, and failed-entry cleanup."""
import sys


class Indexed:
    def __index__(self) -> int:
        return 101


def leaf() -> int:
    return 42


def middle() -> int:
    try:
        return leaf()
    except RecursionError:
        return -1


def reject_current() -> str:
    try:
        sys.setrecursionlimit(2)
    except RecursionError as exc:
        return str(exc)
    return "unexpected change"


old = sys.getrecursionlimit()
print(old)
print(reject_current())
for _ in (0, 1):
    sys.setrecursionlimit(2)
    result = middle()
    sys.setrecursionlimit(old)
    print(result)
print(middle())
sys.setrecursionlimit(Indexed())
print(sys.getrecursionlimit())
sys.setrecursionlimit(old)
try:
    sys.setrecursionlimit(2**80)
except OverflowError as exc:
    print(str(exc))
try:
    sys.setrecursionlimit("5")
except TypeError as exc:
    print(str(exc))
try:
    sys.setrecursionlimit(False)
except ValueError as exc:
    print(str(exc))
try:
    sys.setrecursionlimit(True)
except RecursionError as exc:
    print(str(exc))
try:
    sys.setrecursionlimit(limit=5)
except TypeError as exc:
    print(str(exc))
print(sys.getrecursionlimit())
