"""Captured source roots and yielded owners survive alias mutations."""


class Item:
    number: int
    name: str

    def __init__(self, number: int, name: str) -> None:
        self.number = number
        self.name = name


class Holder:
    values: list[int]
    items: list[Item]

    def __init__(self, values: list[int], items: list[Item]) -> None:
        self.values = values
        self.items = items


def mutate(values: list[int], value: int) -> int:
    if value == 1:
        values.append(3)
    if value == 2:
        values[2] = 30
    return value


def remove_integer(values: list[int]) -> bool:
    values.pop()
    return True


def remove_item(values: list[Item]) -> bool:
    values.pop()
    return True


def clear_optional(values: list[int | None]) -> bool:
    values[0] = None
    return True


def replace_items(holder: Holder, item: Item) -> Item:
    holder.items = [Item(9, "replacement")]
    item.number += 1
    return item


def replace_values(holder: Holder) -> int:
    holder.values = [999]
    return 1000000000000000000000000000000


def replace_mapping(values: dict[str, Item], key: str, item: Item) -> int:
    values[key] = Item(99, "new mapped owner")
    return item.number


def grow_mapping(values: dict[str, int], key: str) -> int:
    values["new"] = 2
    return values[key]


def add_before_iteration(values: dict[str, int]) -> int:
    values["added before iterator"] = 7
    return 10


def grow_set(values: set[int], value: int) -> int:
    values.add(2)
    return value


def main() -> None:
    values = [1, 2]
    changed = [mutate(values, value) for value in values]
    for value in changed:
        print(value)
    huge = 1000000000000000000000000000001
    sole = [huge + 1]
    kept = [value for value in sole if remove_integer(sole)]
    print(kept[0])
    print(len(sole))

    items = [Item(huge + 2, "sole item owner")]
    retained = [item for item in items if remove_item(items)]
    print(retained[0].number)
    print(retained[0].name)
    print(len(items))
    optional: list[int | None] = [huge + 3]
    numbers = [
        value for value in optional if value is not None and clear_optional(optional)
    ]
    print(numbers[0])
    print(optional[0] is None)

    holder = Holder([10, 20], [Item(1, "first"), Item(2, "second")])
    old = [replace_items(holder, item) for item in holder.items]
    print(len(old))
    print(old[0].number)
    print(old[1].number)
    print(holder.items[0].number)
    enumerated = [
        (position, value)
        for position, value in enumerate(holder.values, replace_values(holder))
    ]
    for position, value in enumerated:
        print(position)
        print(value)
    print(holder.values[0])

    shared = Item(3, "shared")
    aliases = [shared for index in range(3)]
    aliases[0].number = 40
    print(aliases[2].number)
    print(shared.number)
    mapped: dict[str, Item] = {
        "one": Item(huge + 4, "first"),
        "two": Item(huge + 5, "second"),
    }
    saved = [replace_mapping(mapped, key, item) for key, item in mapped.items()]
    for number in saved:
        print(number)
    print(mapped["one"].number)

    starting: dict[str, int] = {"original": 3}
    started = [
        (position, key, value)
        for position, (key, value) in enumerate(
            starting.items(), add_before_iteration(starting)
        )
    ]
    for position, key, value in started:
        print(position)
        print(key)
        print(value)

    changing: dict[str, int] = {"first": 1}
    try:
        _failed = [grow_mapping(changing, key) for key in changing]
    except RuntimeError:
        print("mapping changed")
    print(changing["new"])
    unique = {1}
    try:
        _failed = [grow_set(unique, value) for value in unique]
    except RuntimeError:
        print("set changed")
    print(len(unique))


main()
