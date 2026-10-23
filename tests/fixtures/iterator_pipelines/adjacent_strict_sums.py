def totals(left: list[int], right: list[int]) -> None:
    signed = 91
    weighted = 92
    try:
        signed = sum(b - a for a, b in zip(left, right, strict=True))
        weighted = sum(new * (new - old) for old, new in zip(left, right, strict=True))
    except ValueError as error:
        print(error)
    print(signed)
    print(weighted)


def checked(value: int) -> int:
    if value == 0:
        raise ValueError("zero in second sum")
    return value


def failure(left: list[int], right: list[int]) -> None:
    first = -1
    try:
        first = sum(a + b for a, b in zip(left, right, strict=True))
        second = sum(checked(b) for a, b in zip(left, right, strict=True))
        print(second)
    except ValueError as error:
        print(error)
    print(first)


def change_next(values: list[int], value: int) -> int:
    values[1] = value
    return value


def mutate(left: list[int], right: list[int]) -> None:
    before = sum(a * b for a, b in zip(left, right, strict=True))
    after = sum(change_next(right, a) for a, b in zip(left, right, strict=True))
    print(before)
    print(after)
    print(right[1])


huge = 1 << 500
totals([], [])
totals([3], [7])
totals([0, huge], [huge, 0])
totals([1, 2], [3])
totals([1], [3, 4])
failure([1, 2], [0])
failure([1], [0, 2])
failure([1, 2], [0, 4])
mutate([2, 5], [3, 4])
