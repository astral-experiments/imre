from typing import Callable


def target(first: int, second: int) -> int:
    return first + second


def choose() -> Callable[[int, int], int]:
    print("callee")
    return target


def stop() -> str:
    print("argument")
    raise ValueError("stopped")


try:
    print(choose()(stop(), True))
except ValueError:
    print("caught")
