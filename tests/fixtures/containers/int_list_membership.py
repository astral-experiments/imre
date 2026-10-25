def probe(value: int | None, values: list[int]) -> bool:
    return value in values


def take(values: list[int]) -> int:
    return values.pop()


def main() -> None:
    large = int("100000000000000000000000000000000000000")
    values = [-1, 0, large]
    print(probe(None, values))
    print(probe(int(str(large)), values))
    print(probe(large + 1, values))
    print(probe(0, values))
    print(take(values) not in values)
    empty: list[int] = []
    print(probe(0, empty))


main()
