"""Guarded list arithmetic keeps wide integers, range fallbacks, and loop control."""


def show(values: list[int]) -> None:
    print(len(values))
    for value in values:
        print(value)


def groups(values: list[int]) -> list[int]:
    result: list[int] = []
    count = len(values)
    for index in range(0, count, 3):
        if count - index >= 3:
            number = values[index] + values[index + 1] * 45 + values[index + 2] * 2025
            result.extend(divmod(number, 256))
        else:
            result.append(values[index])
    return result


def reversed_pairs(values: list[int]) -> list[int]:
    result: list[int] = []
    for index in range(1, len(values), 2):
        result.append(values[-index] + values[-index - 1])
    return result


def control(values: list[int], stop: int, action: int) -> None:
    result: list[int] = []
    index = 77
    for index in range(0, stop, 3):
        result.append(values[index])
        if action == 0:
            break
        if action == 1:
            index = 1 << 140
            continue
        result.extend(divmod(values[index] + values[index + 1], 256))
    else:
        result.append(-1)
    print(index)
    show(result)


def failing(values: list[int]) -> None:
    result: list[int] = []
    index = 77
    try:
        for index in range(0, len(values), 3):
            result.extend(divmod(values[index] + values[index + 1], 256))
            if index == 3:
                raise ValueError("after append")
        else:  # noqa: PLW0120 - exception versus exhaustion is under test.
            result.append(-1)
    except ValueError as error:
        print(str(error))
    finally:
        print(index)
        show(result)


def dynamic_step(values: list[int], stop: int, step: int) -> None:
    result: list[int] = []
    index = 77
    try:
        for index in range(0, stop, step):
            result.append(values[index] + values[index - 1])
        else:  # noqa: PLW0120 - range failure versus exhaustion is under test.
            result.append(-1)
    except ValueError as error:
        print(str(error))
    print(index)
    show(result)


def bounded_division(value: int) -> None:
    if value >= -4611686018427387904 and value <= 4611686018427387903:
        print(value // 1)
        print(value % 1)
        print(value // 2)
        print(value % 2)
        print(value // 256)
        print(value % 256)
        print(value // 2305843009213693952)
        print(value % 2305843009213693952)


def dynamic_division(value: int, divisor: int) -> None:
    # Supported Python versions differ in zero-error wording; check the kind.
    try:
        print(value // divisor)
    except ZeroDivisionError:
        print("floor zero")
    try:
        print(value % divisor)
    except ZeroDivisionError:
        print("modulo zero")


for values in [
    [],
    [0],
    [1, 2, 3, 4],
    [-1, -2, -3, 0, 255, 256],
    [-2147483648, 2147483647, 0],
    [-2147483649, 2147483648, 0],
    [1 << 150, -(1 << 160), 19],
]:
    show(groups(values))
values = [1, 2, 3, 4, 5, 6]
show(reversed_pairs(values))
show(reversed_pairs([1 << 150, 2, 3, 4, 5, 6]))
for stop in [0, -1, 6]:
    control(values, stop, 2)
control(values, 1 << 150, 0)
control(values, 6, 1)
failing(values)
failing([1 << 150, 2, 3, -(1 << 160), 5, 6])
dynamic_step(values, -4, -1)
dynamic_step(values, 6, 0)
dynamic_step(values, 6, 1 << 150)
larger = [index % 45 for index in range(257)]
control(larger, 0, 0)
control(larger, 3, 0)
control(larger, 1 << 150, 0)
for value in [
    -4611686018427387905,
    -4611686018427387904,
    -4611686018427387903,
    -257,
    -256,
    -255,
    -1,
    0,
    1,
    255,
    256,
    257,
    4611686018427387902,
    4611686018427387903,
    4611686018427387904,
    -(1 << 150),
    1 << 150,
]:
    bounded_division(value)
    for divisor in [1, 2, 256, 2305843009213693952, -1, -256, 0, 3, 1 << 150]:
        dynamic_division(value, divisor)
