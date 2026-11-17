def outer(x: int) -> int:
    if x == 0:
        return 0

    def inner(y: int) -> int:
        return outer(y - 1)

    return inner(x) + 1


print(outer(5))
