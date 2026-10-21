"""Local optional facts survive only the paths and bindings that justify them."""


def positive(value: int | None) -> bool:
    return value is not None and value > 0


def absent_or_small(value: int | None) -> bool:
    return value is None or value < 3


def early_return(value: int | None) -> int:
    if value is None:
        return -1
    return value + 1


def reversed_guard(value: int | None) -> int:
    if None is value:
        return -1
    if None is not value:
        return value + 2
    return -2


def conjuncts(left: int | None, right: int | None) -> int:
    if left is not None and right is not None:
        return left + right
    return -1


def disjuncts(left: int | None, right: int | None) -> int:
    if left is None or right is None:
        return -1
    return left + right


def negation(left: int | None, right: int | None) -> int:
    if not (left is not None and right is not None):
        return -1
    return left + right


def reassign(value: int | None) -> int:
    if value is not None:
        value = None
        value = 17
        if not (value is None):
            return value
    return -1


def increment(value: int | None) -> int:
    if value is None:
        return -1
    value += 1
    value *= 2
    return value


def concatenate(value: str | None) -> str:
    if value is None:
        return "missing"
    value += "!"
    return value


def guarded_while(value: int | None) -> int:
    total = 0
    while value is not None and value > 0:
        total += value
        value = None
    return total


def guarded_for(value: int | None) -> int:
    total = 0
    if value is not None:
        for index in range(2):
            if value is None:
                value = index
            if value is not None:
                total += value
            value = None
    return total


def unchanged_binding(value: int | None) -> int:
    if value is None:
        return 0
    total = 0
    for index in range(3):
        total += value
    return total


def copied_element(values: list[int | None]) -> int:
    value = values[0]
    if value is None:
        return -1
    values[0] = None
    return value + 1


def clear(values: list[int | None]) -> bool:
    values[0] = None
    return True


def retained_argument(value: int | None, values: list[int | None]) -> int:
    if value is not None and clear(values):
        return value
    return -1


def main() -> None:
    print(positive(None))
    print(positive(-1))
    print(positive(4))
    print(absent_or_small(None))
    print(absent_or_small(2))
    print(absent_or_small(4))
    print(early_return(None))
    print(early_return(7))
    print(reversed_guard(None))
    print(reversed_guard(7))
    print(conjuncts(None, 3))
    print(conjuncts(2, None))
    print(conjuncts(2, 3))
    print(disjuncts(None, 3))
    print(disjuncts(2, None))
    print(disjuncts(2, 3))
    print(negation(None, 3))
    print(negation(2, None))
    print(negation(2, 3))
    print(reassign(None))
    print(reassign(4))
    print(increment(None))
    print(increment(4611686018427387903))
    print(concatenate(None))
    print(concatenate("present"))
    print(guarded_while(None))
    print(guarded_while(-1))
    print(guarded_while(7))
    print(guarded_for(None))
    print(guarded_for(7))
    print(unchanged_binding(None))
    print(unchanged_binding(7))
    values: list[int | None] = [1000000000000000000000000000000]
    print(copied_element(values))
    print(copied_element(values))
    values[0] = 1000000000000000000000000000001
    print(retained_argument(values[0], values))
    print(values[0] is None)


main()
