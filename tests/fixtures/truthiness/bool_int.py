def mark(integer: int) -> int:
    print("right")
    print(integer)
    return integer


def conjunction(flag: bool, integer: int) -> bool | int:
    return flag and mark(integer)


def disjunction(flag: bool, integer: int) -> bool | int:
    return flag or mark(integer)


def reverse_and(integer: int, flag: bool) -> bool | int:
    return integer and flag


def reverse_or(integer: int, flag: bool) -> bool | int:
    return integer or flag


def nullable(flag: bool | None, integer: int) -> bool | int | None:
    return flag and integer


def show(value: bool | int | None) -> None:
    print(repr(value))
    print(isinstance(value, bool))


value = True and 1
show(value)
for flag in [False, True]:
    for integer in [0, 1, -99, 1 << 100]:
        show(conjunction(flag, integer))
        show(disjunction(flag, integer))
        show(reverse_and(integer, flag))
        show(reverse_or(integer, flag))
show(nullable(None, 1))
show(nullable(False, 1))
show(nullable(True, 1))
