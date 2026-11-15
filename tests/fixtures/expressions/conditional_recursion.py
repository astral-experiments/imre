def first(value: int) -> int:
    return second(value - 1) if value > 0 else 10


def second(value: int) -> int:
    return first(value - 1) if value > 0 else 20


print(first(4))
print(first(3))
