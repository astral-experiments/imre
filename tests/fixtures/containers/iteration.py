"""Iteration captures roots and counters while observing live list contents."""


class Item:
    number: int
    name: str

    def __init__(self, number: int, name: str) -> None:
        self.number = number
        self.name = name


class Holder:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def numbers(log: list[int]) -> list[int]:
    log.append(1)
    return [10, 20, 30]


def captured_start(holder: Holder, log: list[int]) -> int:
    log.append(2)
    holder.values = [999]
    return 1000000000000000000000000000000


def list_changes() -> None:
    values = [1, 2]
    alias = values
    value = -1
    for value in values:
        print(value)
        if value == 1:
            alias.append(3)
        if value == 2:
            alias[2] = 30
    print(value)

    values = [1, 2, 3, 4]
    for value in values:
        print(value)
        values.pop()
    print(len(values))
    print(value)

    values = [1, 2, 3]
    for value in values:
        print(value)
        if value == 1:
            values.pop()
            values.pop()
            values.append(9)

    values = [4, 5, 6]
    original = values
    for value in values:
        print(value)
        values = [99]
        if value == 4:
            original[1] = 50
    print(values[0])
    print(value)

    empty: list[int] = []
    value = 1000000000000000000000000000007
    for value in empty:
        print("unreachable")
    print(value)


def nested_control() -> None:
    value = -1
    for value in [1, 2, 3]:
        print(value)
        for value in [7, 8, 9]:
            if value == 8:
                break
            print(value)
        print(value)
        if value == 8:
            continue
        print("unreachable")
    print(value)

    log: list[int] = []
    for value in numbers(log):
        print(value)
        if value == 20:
            break
    print(len(log))
    print(log[0])


def enumeration() -> None:
    log: list[int] = []
    holder = Holder(numbers(log))
    counter = -1
    value = -1
    for counter, value in enumerate(holder.values, captured_start(holder, log)):
        print(counter)
        print(value)
        counter = -100
        if value == 20:
            continue
        print(holder.values[0])
    print(counter)
    print(value)
    for event in log:
        print(event)

    start = 4611686018427387903
    for counter, value in enumerate([1, 2, 3], start):
        print(counter)
        print(value)
        start = -100
    for counter, value in enumerate([4, 5], -4611686018427387905):
        print(counter)
        print(value)

    # The iterator counter is independent of either visible target binding.
    for counter, value in enumerate([1, 2, 3], 7):
        print(counter)
        counter = 1000000000000000000000000000000
        value = 0
        continue
    print(counter)
    print(value)

    for pair in enumerate(["one", "two"]):
        print(pair[0])
        print(pair[1])
    for counter, counter in enumerate([10, 20], 40):
        print(counter)
    empty: list[int] = []
    counter = 99
    value = 88
    for counter, value in enumerate(empty, 1000000000000000000000000000000):
        print("unreachable")
    print(counter)
    print(value)


def first(items: list[Item]) -> Item:
    for item in items:
        return item
    return Item(0, "empty")


def owned_targets() -> None:
    values = [1000000000000000000000000000001]
    value = 0
    for value in values:
        values.pop()
        print(value)
    print(value)

    texts = ["a long string whose final list owner is removed"]
    text = "before"
    for text in texts:
        texts.pop()
        print(text)
    print(text)

    items = [Item(1000000000000000000000000000002, "retained object")]
    for item in items:
        items.pop()
        print(item.number)
        print(item.name)
    returned = first([Item(1000000000000000000000000000003, "returned object")])
    print(returned.number)
    print(returned.name)

    rows = [[1000000000000000000000000000004]]
    for row in rows:
        rows.pop()
        row.append(9)
        print(row[0])
        print(row[1])

    optional: list[int | None] = [None, 1000000000000000000000000000005, None]
    for maybe in optional:
        if maybe is None:
            print(None)
        else:
            print(maybe + 1)
    for flag in [True, False]:
        print(flag)
    nothing: list[None] = [None, None]
    for absent in nothing:
        print(absent)


def unpacking() -> None:
    left = 1000000000000000000000000000001
    right = 1000000000000000000000000000002
    left, right = right, left
    print(left)
    print(right)
    [left, right] = (right, left)
    print(left)
    print(right)
    left, left = (left + 1, left + 2)  # noqa: PLW0128 - repeated targets are intentional
    print(left)

    first_values = [11]
    second_values = [22]
    first_values, second_values = second_values, first_values
    print(first_values.pop())
    print(second_values.pop())

    log: list[int] = []
    holder = Holder([1000000000000000000000000000006])
    old_values, marker = (holder.values, captured_start(holder, log))
    print(old_values[0])
    print(marker)
    print(holder.values[0])

    pairs: list[tuple[int, str]] = [(1, "one"), (2, "two")]
    for number, name in pairs:
        print(number)
        print(name)
    for [number, name] in pairs:
        print(number)
        print(name)

    nested: list[tuple[str, tuple[int, list[int]]]] = [
        ("retained tuple", (1000000000000000000000000000007, [8]))
    ]
    for index, (name, (number, payload)) in enumerate(nested, 9):
        nested.pop()
        print(index)
        print(name)
        print(number)
        print(payload[0])
    name, (number, payload) = ("nested assignment", (11, [12]))
    print(name)
    print(number)
    print(payload[0])

    maybe: int | None = 1
    if maybe is not None:
        maybe, marker = (None, 13)
        print(maybe is None)
        print(marker)


list_changes()
nested_control()
enumeration()
owned_targets()
unpacking()
