"""Existing range, sequence, dictionary, set and enumerate sources compose."""


def total(values: list[int]) -> int:
    result = 0
    for value in values:
        result += value
    return result


def main() -> None:
    huge = 1000000000000000000000000000001
    for value in [value for value in range(huge, huge + 3)]:
        print(value)
    for value in [value for value in range(3, -4, -2) if value != 1]:
        print(value)
    for value in [
        value for value in range(-4611686018427387903, -4611686018427387907, -1)
    ]:
        print(value)

    names = ["first", "é", "last"]
    for position, name in [
        (position, name) for position, name in enumerate(names, huge)
    ]:
        print(position)
        print(name)
    for pair in [pair for pair in enumerate(names)]:
        print(pair[0])
        print(pair[1])

    values: dict[str, int] = {"first": huge, "second": 0, "third": -huge}
    keys = [key for key in values]
    for key in keys:
        print(key)
    records = [
        (position, key, value)
        for position, (key, value) in enumerate(values.items(), -huge)
    ]
    for position, key, value in records:
        print(position)
        print(key)
        print(value)
    transformed = {key: value + 1 for key, value in values.items() if key != "second"}
    for key, value in transformed.items():
        print(key)
        print(value)

    # Set traversal order is deliberately not observed.
    unique = {value % 3 for value in range(10)}
    print(len(unique))
    print(0 in unique)
    print(4 not in unique)
    print(total([value * 2 for value in unique]))
    print(total([position for position, value in enumerate(unique, 4)]))
    lengths = {len(name) for name in {"a", "bbb", "cc"}}  # noqa: PLC0208 — test set iteration
    print(len(lengths))
    print(total([value for value in lengths]))
    named = {name for name in names if name != "é"}
    print(len(named))
    print("first" in named)
    print("é" not in named)

    empty_names: list[str] = []
    empty_values: dict[str, int] = {}
    empty_unique: set[str] = set()
    print(len([name for name in empty_names]))
    print(len({key: value for key, value in empty_values.items()}))
    print(len({name for name in empty_unique}))
    print(len([value for value in range(0) for other in range(1 // 0)]))


main()
