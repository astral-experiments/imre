def update(size: int, index: int, value: int) -> None:
    items = [-1] * size
    items[0] = 2**100
    try:
        items[index] = value
        print(items[index])
    except IndexError:
        print("outside")
    print(f"{items[0]} {items[-1]} {len(items)}")


def resized(items: list[int]) -> int:
    items.append(71)
    return -1


def aliases() -> None:
    values = [0] * 2
    alias = values
    alias.append(8)
    values[resized(alias)] = 9
    print(f"{values[2]} {values[3]}")


def shared_args(a: list[int], b: list[int]) -> None:
    a.append(7)
    i = 0
    while i < len(a):
        a[i] = b[-1]
        i += 1
    print(f"{a[0]} {b[-1]}")


for at in [0, -1, 4, -5, 2**91]:
    update(4, at, 2**85)
aliases()
both = [2, 3]
shared_args(both, both)
