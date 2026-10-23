from typing import Callable


def identity(value: int) -> int:
    return value


class Offset:
    amount: int

    def __init__(self, amount: int) -> None:
        self.amount = amount

    def convert(self, value: int) -> int:
        return self.amount + value

    def accept(self, value: int) -> bool:
        self.amount += 1
        print(value)
        if value < 0:
            raise ValueError("negative row")
        return value != 0


def report(
    rows: list[int], accept: Callable[[int], bool], convert: Callable[[int], int]
) -> int:
    total = 0
    for value in map(convert, filter(accept, rows)):
        total += value
    return total


def argument(callback: Callable[[int], int], value: int) -> int:
    return value


def rebound(rows: list[int], convert: Callable[[int], int]) -> int:
    total = 0
    for row in rows:
        # The first call still uses the bound receiver captured before its argument.
        total += convert(argument((convert := identity), row))
    return total


def main() -> None:
    rows = [0, 20, 3]
    state = Offset(1 << 90)
    print(report(rows, state.accept, identity))
    print(report(rows, state.accept, state.convert))
    print(report(rows, state.accept, Offset(-100).convert))
    try:
        print(report([0, -1, 42], state.accept, state.convert))
    except ValueError as error:
        print(error)
    print(rebound(rows[1:], state.convert))


main()
