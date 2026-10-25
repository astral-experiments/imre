"""Calling a retained typed original must execute its body, even after clearing."""
from typing import Callable, clear_overloads, get_overloads, overload


def record(items: list[int]) -> int:
    items.append(41)
    return items[-1] + 1


def main() -> None:
    overload(record)
    originals = get_overloads(record)
    clear_overloads()
    callback: Callable[[list[int]], int] = originals[0]
    items: list[int] = []
    print(callback(items))
    print(repr(items))
    print(callback is record)


main()
