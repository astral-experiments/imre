def visit(key: int, events: list[int]) -> int:
    events.append(key)
    if key == 3:
        raise ValueError("bad key")
    return key


def parity(key: int) -> int:
    return key % 2


def main() -> None:
    print(set(range(6)) == {0, 1, 2, 3, 4, 5})
    print(set(range(9, 0, -3)) == {3, 6, 9})
    print(len(set(range(0))))
    print(set(map(parity, range(6))) == {0, 1})
    print(set(x for x in ("a", "b", "a")) == {"b", "a"})

    events: list[int] = []
    try:
        print(len(set(visit(i, events) for i in range(5))))
    except ValueError as error:
        print(error)
    print(events)
    try:
        print(len(set(range(2, 8, 0))))
    except ValueError as error:
        print(error)


main()
