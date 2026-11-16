import sys
from functools import singledispatch
from typing import Callable, get_overloads, overload

import operations
from operations import decrement as down


def from_wrapper(value: int) -> int:
    caller = sys._getframemodulename(1)
    if caller is None:
        return 0
    return value if caller == "operations" else -value


def main() -> None:
    selected: Callable[[int], int] = operations.increment
    print(selected(1 << 100))
    print(operations.callback(41), operations.callback is operations.increment)
    selected = down
    overload(selected)
    registered = get_overloads(operations.decrement)
    assert registered[0] is down
    assert registered[0].__module__ == "operations"
    assert registered[0].__qualname__ == "decrement"
    print(operations.apply(selected, -(1 << 100)))
    print(operations.choose(True)(7))
    print(sum(map(operations.increment, [1, 2, 3])))

    dispatch = singledispatch(operations.increment)
    dispatch.register(int, from_wrapper)
    print(dispatch(7))
    print(from_wrapper(7))
    depth = 0
    while sys._getframemodulename(depth) is not None:
        depth += 1
    old_limit = sys.getrecursionlimit()
    for extra in (1, 2, 1):
        sys.setrecursionlimit(depth + extra)
        try:
            value = dispatch(7)
        except RecursionError:
            value = -9
        finally:
            sys.setrecursionlimit(old_limit)
        print(value)


main()
