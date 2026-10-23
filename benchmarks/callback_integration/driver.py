from collections.abc import Callable
from sys import argv

from integration import trapezoidal_area


def constant(x: float) -> float:
    return 5.0


def quadratic(x: float) -> float:
    return 9 * (x * x)


def select(which: int) -> Callable[[float], float]:
    return constant if which == 0 else quadratic


def repeat(which: int, left: float, right: float, steps: int, rounds: int) -> float:
    callback = select(which)
    total = 0.0
    for _ in range(rounds):
        total += trapezoidal_area(callback, left, right, steps)
    return total


print(repeat(int(argv[1]), float(argv[2]), float(argv[3]), int(argv[4]), int(argv[5])))
