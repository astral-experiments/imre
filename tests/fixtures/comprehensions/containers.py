"""Contextual element types and optional facts cover every ownership encoding."""


class Item:
    number: int
    name: str

    def __init__(self, number: int, name: str) -> None:
        self.number = number
        self.name = name


class Rows:
    rows: list[list[str]]

    def __init__(self, count: int) -> None:
        self.rows = [[] for index in range(count)]


class Shadowed:
    values: list[int]

    def __init__(self, count: int) -> None:
        self.values = [self for self in range(count)]


def flag(value: int) -> bool | None:
    if value == 0:
        return None
    return value == 2


def nullable(value: int) -> int | None:
    if value == 0:
        return None
    return value - 1


def optional_item(value: int) -> Item | None:
    if value == 0:
        return None
    return Item(1000000000000000000000000000000 + value, str(value))


def empty_rows() -> list[list[str]]:
    return [[] for index in range(2)]


def empty_maps() -> dict[int, dict[str, int]]:
    return {index: {} for index in range(2)}


def consume(rows: list[list[str]]) -> int:
    rows[0].append("contextual argument")
    return len(rows[0])


def narrowed(offset: int | None, values: list[int | None]) -> list[int]:
    if offset is None:
        return []
    return [value + offset for value in values if value is not None if value >= 0]


def outside_fact(value: int | None) -> int:
    assert value is not None
    for turn in range(2):
        values = [value for value in [None]]
        print(value + len(values))
    return value


def main() -> None:
    flags: list[bool | None] = [flag(index) for index in range(3)]
    for flag_value in flags:
        print(flag_value is None)
        if flag_value is not None:
            print(flag_value)
    numbers: list[int | None] = [index for index in range(3)]
    numbers.append(None)
    for value in narrowed(3, numbers):
        print(value)
    present = [
        value
        for value in [nullable(index) for index in range(4)]
        if value is not None and value < 2
    ]
    for value in present:
        print(value)
    expanded = [
        value + inner
        for value in numbers
        if value is not None
        for inner in range(value)
    ]
    for value in expanded:
        print(value)
    reshaped = [
        value + 1
        for value in numbers
        if value is not None
        for value in numbers
        if value is not None
    ]
    print(len(reshaped))

    items: list[Item | None] = [optional_item(index) for index in range(3)]
    kept = [item for item in items if item is not None]
    items[1] = None
    for item in kept:
        print(item.number)
        print(item.name)
    tuples: list[tuple[bool | None, int | None, Item | None]] = [
        (flag(index), nullable(index), optional_item(index)) for index in range(3)
    ]
    for maybe_flag, number, optional_record in tuples:
        print(maybe_flag is None)
        if maybe_flag is not None:
            print(maybe_flag)
        print(number is None)
        if number is not None:
            print(number)
        if optional_record is not None:
            print(optional_record.number)

    optional_maps: dict[int, bool | None] = {index: flag(index) for index in range(3)}
    for key, mapped_flag in optional_maps.items():
        print(key)
        print(mapped_flag is None)
        if mapped_flag is not None:
            print(mapped_flag)
    strings: list[str | None] = [text for text in ["", "owned string"]]
    strings.append(None)
    print(len([text for text in strings if text is not None]))

    rows: list[list[str]] = [[] for index in range(2)]
    rows[0].append("independent")
    print(len(rows[1]))
    print(len(empty_rows()))
    print(consume([[] for index in range(1)]))
    mapping = empty_maps()
    mapping[0]["value"] = 7
    print(len(mapping[1]))
    nested: list[list[list[int]]] = [[[] for inner in range(2)] for outer in range(2)]
    nested[0][0].append(8)
    print(len(nested[0][1]))
    print(len(nested[1][0]))
    holder = Rows(2)
    holder.rows[0].append("constructor")
    print(len(holder.rows[1]))
    groups: list[set[str]] = [set() for index in range(2)]
    groups[0].add("one")
    print(len(groups[1]))
    inferred = [[] for index in range(2)]
    inferred[0].append(4)
    print(inferred[0][0])
    inferred_mapping = {index: [] for index in range(2)}
    inferred_mapping[1].append(5)
    print(inferred_mapping[1][0])
    print(Shadowed(3).values[2])
    print(outside_fact(7))


main()
