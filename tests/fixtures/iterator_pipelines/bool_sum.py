def source(items: list[int]) -> list[int]:
    print("source")
    return items


def initial(items: list[int], fail: bool) -> int:
    print("start")
    if fail:
        raise ValueError("start failed")
    items.append(3)
    return 1000000000000000000000000000000


def passes(value: int) -> bool:
    print(value)
    if value < 0:
        raise ValueError("projection failed")
    return value > 1


items = [1, 2]
total: int = sum((passes(n) for n in source(items)), start=initial(items, False))
print(total)
print(sum(n > 0 for n in range(0)))
print(sum((False for n in [1]), -4))
for fail in [True, False]:
    try:
        print(sum((passes(n) for n in source([1, -1, 2])), initial([], fail)))
    except ValueError as error:
        print(str(error))
try:
    print(sum((passes(n) for n in range(0, 2, 0)), initial([], False)))
except ValueError as error:
    print(str(error))
