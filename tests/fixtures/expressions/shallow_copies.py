"""A slice copies list slots while retaining the identities of referenced values."""


class Item:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


def main() -> None:
    integers = [1000000000000000000000000000000, 2, 3]
    copied = integers[:]
    integers[0] = 0
    copied.append(4)
    print(copied[0])
    print(len(integers))
    print(len(copied))

    nested = [[1], [2]]
    nested_copy = nested[::-1]
    nested_copy[0].append(3)
    print(nested[1][1])
    nested_copy[0] = []
    print(len(nested[1]))

    items = [Item(10), Item(20)]
    items_copy = items[::-1]
    items_copy[0].value = 99
    print(items[1].value)
    print(items_copy[0] == items[1])
    items.pop()
    print(items_copy[0].value)

    flags: list[bool | None] = [None, False, True]
    for flag in flags[::-1]:
        print("none" if flag is None else str(flag))
    absent: list[None] = [None, None]
    print(len(absent[::2]))
    optional_words: list[str | None] = [None][:]
    optional_words.append("word")
    for word in optional_words:
        print("none" if word is None else word)
    optional_items: list[Item | None] = [None, items[0]]
    optional_copy = optional_items[::-1]
    item = optional_copy[0]
    if item is not None:
        item.value = 77
    print(items[0].value)
    print(optional_copy[1] is None)

    records: list[tuple[int, str]] = [(1, "first"), (2, "second")]
    for number, text in records[::-1]:
        print(number)
        print(text)
    mappings: list[dict[str, int]] = [{"value": 1}]
    mapping_copy = mappings[:]
    mapping_copy[0]["value"] = 8
    print(mappings[0]["value"])
    sets: list[set[str]] = [{"first"}]
    set_copy = sets[:]
    set_copy[0].add("second")
    print(len(sets[0]))


main()
