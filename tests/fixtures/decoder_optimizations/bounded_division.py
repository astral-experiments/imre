def divide(value: int) -> None:
    if 0 <= value < 20:
        print(value // 10)
        print(value % 10)
        print(sum(divmod(value, 10)))
    else:
        print(value // 10)
        print(value % 10)
    if 0 <= value <= 4611686018427387903:
        print(value // 2305843009213693953)
        print(value % 2305843009213693953)


def rebind(value: int) -> None:
    # The numerator was captured before evaluating the divisor's assignment.
    if 0 <= value < 20:
        print(value // (value := 10))
        print(value)


for value in [
    -(1 << 100),
    -21,
    -20,
    -19,
    -1,
    0,
    1,
    9,
    10,
    11,
    19,
    20,
    21,
    2305843009213693952,
    2305843009213693953,
    2305843009213693954,
    4611686018427387903,
    4611686018427387904,
    1 << 100,
]:
    divide(value)
    rebind(value)
