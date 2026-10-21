from typing import Callable

import operations
from operations import decrement as down


def main() -> None:
    selected: Callable[[int], int] = operations.increment
    print(selected(1 << 100))
    selected = down
    print(operations.apply(selected, -(1 << 100)))
    print(operations.choose(True)(7))
    print(sum(map(operations.increment, [1, 2, 3])))


main()
