def key(row: tuple[int, str, int]) -> tuple[int, str]:
    print(row[2])
    return (row[0], row[1])


def failing(row: tuple[int, str, int]) -> tuple[int, str]:
    if row[2] == 2:
        raise ValueError("key failed")
    return (row[0], row[1])


def main() -> None:
    rows = [
        (1, "b", 0), (1, "a", 1), (1, "a", 2), (0, "z", 3),
        (2, "a", 4), (1, "a", 5), (0, "z", 6), (1, "b", 7), (0, "z", 8),
    ]
    for row in sorted(rows, key=key, reverse=True):
        print(row[2])
    try:
        sorted(rows, key=failing)
    except ValueError:
        print("key failed")
    for row in rows:
        print(row[2])
    nested = [(1, ("é", b"b"), None), (1, ("a", b"a"), None), (-100000000000000000000000000, ("z", b"z"), None)]
    for item in sorted(nested):
        print(item[0])
        print(item[1][0])
    print(nested[1] < nested[0])
    print(nested[1] <= nested[1])
    print(nested[0] > nested[1])
    print(nested[0] >= nested[0])
    print(nested[1] != nested[0])
    print(sorted([(True, 1), (False, 2)])[0][1])
    print(((), 1) < ((), 2))


main()
