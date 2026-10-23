def private(xs: list[int]) -> list[int]:
    ys: list[int] = []
    cursor = 0
    while cursor < len(xs):
        ys.append(cursor)
        cursor += 1
    return ys


def maybe_alias(xs: list[int], shared: bool) -> int:
    ys: list[int] = []
    if shared:
        ys = xs
    cursor = 0
    while cursor < len(xs):
        if cursor == 0:
            ys.append(1)
        cursor += 1
    return cursor


print(bytes(private([1, 1, 1])))
print(maybe_alias([0], False))
print(maybe_alias([0], True))
