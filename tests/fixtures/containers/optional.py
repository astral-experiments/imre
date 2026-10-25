class Item:
    name: str

    def __init__(self, name: str) -> None:
        self.name = name


def find(items: list[Item], name: str) -> Item | None:
    for index in range(len(items)):
        item = items[index]
        if item.name == name:
            return item
    return None


def show(item: Item | None) -> None:
    if item is None:
        print("missing")
    else:
        print(item.name)


def maybe_text(flag: bool) -> str | None:
    if flag:
        return "text"
    return None


def maybe_number(flag: bool) -> int | None:
    if flag:
        return 100000000000000000000000000000000000000000000000000
    return None


def add_one(value: int | None) -> int:
    return value + 1


def main() -> None:
    items: list[Item] = [Item("found")]
    result = find(items, "found")
    items[0] = Item("replacement")
    show(result)
    show(find(items, "missing"))
    show(None)
    show(Item("direct"))

    options: list[Item | None] = [None, Item("kept")]
    alias = options
    saved = alias[-1]
    options[-1] = None
    if saved is not None:
        print(saved.name)
    else:
        print("unexpected missing")
    options.append(Item("popped"))
    popped = options.pop()
    if popped is None:
        print("unexpected pop")
    else:
        print(popped.name)
    empty = options[0]
    if empty is None:
        print("empty")

    text = maybe_text(True)
    if text is not None:
        print(text + " suffix")
    number = maybe_number(True)
    if number is None:
        print(0)
    else:
        print(number + 1)
    absent = maybe_number(False)
    if absent is not None:
        print(absent)
    else:
        print("no number")

    option_rows: list[list[str] | None] = [None, ["row"]]
    row = option_rows.pop()
    if row is not None:
        row.append("owned")
        print(row[-1])
    optional_pair: tuple[str | None, Item | None] = (None, Item("tuple"))
    pair_item = optional_pair[-1]
    if pair_item is not None:
        print(pair_item.name)

    values: dict[str, int | None] = {"one": 1, "empty": None}
    for key in ["one", "empty"]:
        try:
            print(values[key] + 1)
        except TypeError as error:
            print(str(error))
        try:
            print(add_one(values.get(key)))
        except TypeError as error:
            print(str(error))
    if values.get("one") is not None:
        print(values.get("one") + 1)
        del values["one"]
        try:
            print(values.get("one") + 1)
        except TypeError as error:
            print(str(error))


main()
