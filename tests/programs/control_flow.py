def choose(flag: bool) -> int:
    if flag:
        value = 1
    else:
        value = 2
    return value


def early(items: list[int], flag: bool) -> list[int]:
    scratch = [100000000000000000000000000000]
    if flag:
        return items
    return scratch


def main() -> None:
    print(choose(True))
    print(choose(False))
    i = 99
    for i in range(0):
        print(999)
    print(i)
    for i in range(5, -2, -2):
        if i == 3:
            continue
        print(i)
        if i == -1:
            break
    print(i)
    total = 0
    for x in range(4):
        for y in range(4):
            if y == 1:
                continue
            if y == 3:
                break
            total += x * y
    print(total)
    value = 0
    while value < 6:
        value += 1
        if value == 2:
            continue
        elif value == 5:
            break
        else:
            print(value)
    print(value)
    for large in range(100000000000000000000, 100000000000000000003):
        print(large)
    items = [1]
    kept = early(items, True)
    items = [2]
    print(kept[0])
    kept = early(items, False)
    print(kept[0])
    assert len(kept) == 1


main()
