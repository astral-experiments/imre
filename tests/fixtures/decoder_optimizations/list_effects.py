"""Aliases and effects cannot invalidate a specialized source buffer or its bounds."""


def show(values: list[int]) -> None:
    print(len(values))
    for value in values:
        print(value)


def aliased(values: list[int], use_alias: bool) -> None:
    # A capacity hint must preserve an existing prefix on a distinct output.
    result = [99]
    if use_alias:
        result = values
    count = len(values)
    for index in range(0, count, 3):
        number = values[index] + values[index + 1] * 45 + values[index + 2] * 2025
        # extend lowers through a hidden receiver local, which still aliases values.
        result.extend(divmod(number, 256))
    show(values)
    show(result)


def mutate(values: list[int], log: list[int]) -> int:
    log.append(1)
    values[3] += 1 << 150
    values.append(99)
    return 1


def body_effects(values: list[int]) -> None:
    alias = values
    result: list[int] = []
    log: list[int] = []
    for index in range(0, len(values), 3):
        result.append(values[index] + mutate(alias, log) + values[index + 1])
    show(result)
    show(values)
    show(log)


def rebind(values: list[int]) -> None:
    result: list[int] = []
    try:
        for index in range(0, len(values), 3):
            result.append(values[index] + values[index + 1])
            values = [99, 98, 97, 1 << 150, 95, 94]
    except IndexError as error:
        print(str(error))
    show(result)
    show(values)


def branch_alias(values: list[int]) -> None:
    other: list[int] = []
    destination: list[int] = []
    for index in range(0, len(values), 3):
        if index == 0:
            destination = other
        else:
            destination = values
        number = values[index] + values[index + 1] * 45 + values[index + 2] * 2025
        destination.extend(divmod(number, 256))
    show(values)
    show(other)


def marker(log: list[int], value: int) -> int:
    log.append(value)
    return value


def bounds(values: list[int], stop: int) -> None:
    result: list[int] = []
    log: list[int] = []
    try:
        for index in range(0, stop, 3):
            result.append(
                values[index] + marker(log, 10) + values[index + 1] + marker(log, 20)
            )
    except IndexError as error:
        print(str(error))
    show(result)
    show(log)


def guarded_bounds(values: list[int], stop: int) -> None:
    result: list[int] = []
    try:
        for index in range(0, stop, 3):
            result.append(values[index])
            result.append(values[index + 1])
    except IndexError as error:
        print(str(error))
    show(result)


def append_locals(result: list[int], first: int, second: int, third: int) -> None:
    result.append(first)
    result.append(second)
    result.append(third)


def append_retained() -> list[int]:
    result = [11]
    value = 1 << 150
    append_locals(result, value, value, -value)
    return result


def append_alias_growth(values: list[int]) -> None:
    alias = values
    for _ in range(8):
        first = values[0]
        second = values[1]
        alias.append(first)
        alias.append(second)
    show(values)


def raising_value(values: list[int]) -> int:
    print(len(values))
    raise ValueError("second append")


def append_observations() -> None:
    result: list[int] = []
    value = -7
    try:
        result.append(value)
        result.append(raising_value(result))
    except ValueError as error:
        print(str(error))
    show(result)
    result.append(value)
    result.append(len(result))
    result.append(value)
    result.append(value := 17)
    print(value)
    show(result)
    other = [99]
    result.append(value)
    other.append(value)
    show(result)
    show(other)


for use_alias in [False, True]:
    aliased([1, 2, 3, 4, 5, 6], use_alias)
    aliased([1 << 150, 2, 3, 4, 5, 6], use_alias)
body_effects([1, 2, 3, 4, 5, 6])
rebind([1, 2, 3, 4, 5, 6])
branch_alias([1, 2, 3, 4, 5, 6, 7, 8, 9])
for values in [[], [7], [7, 8, 9, 10]]:
    bounds(values, 6)
    guarded_bounds(values, 6)

# A direct append destination alias must also keep current storage and length.
values = [1, 2, 3, 4, 5, 6]
for index in range(0, len(values), 3):
    values.append(values[index] + values[index + 1])
show(values)

values = [19]
append_locals(values, -2, 0, 3)
show(values)
show(append_retained())
append_alias_growth([-1, 2])
append_alias_growth([1 << 150, -(1 << 160)])
append_observations()
