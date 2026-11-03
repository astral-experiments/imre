def rise(keys: list[int], other: list[int]) -> None:
    pos = len(keys) - 1
    while pos > 0:
        parent = (pos - 1) // 2
        if keys[parent] > keys[pos]:
            break
        keys[pos] = keys[parent]
        other[pos] = other[parent]
        pos = parent


def resize(values: list[int]) -> int:
    values.append(31)
    return len(values) - 1


def changed(values: list[int]) -> None:
    pos = len(values) - 1
    while pos > 0:
        values[pos] = values[resize(values)]
        pos -= 1


def main() -> None:
    a = [10**40, 10**50, 10**60, 10**70]
    rise(a, a)
    print(str(a))
    b = [0, 1, 2, 3]
    short = [12]
    try:
        rise(b, short)
    except IndexError:
        print(str(b))
        print(str(short))
    changed(b)
    print(str(b))


main()
