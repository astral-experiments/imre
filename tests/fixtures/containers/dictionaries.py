"""Dictionary values, ordering, and captured ownership match Python's behavior."""


class Item:
    name: str
    number: int

    def __init__(self, name: str, number: int) -> None:
        self.name = name
        self.number = number


class NumberHolder:
    values: dict[int, int]

    def __init__(self, values: dict[int, int]) -> None:
        self.values = values


class ListHolder:
    values: dict[str, list[int]]

    def __init__(self, values: dict[str, list[int]]) -> None:
        self.values = values


def maybe_number(flag: bool) -> int | None:
    if flag:
        return 17
    return None


def maybe_item(flag: bool) -> Item | None:
    if flag:
        return Item("fallback", 18)
    return None


def maybe_values(flag: bool) -> list[int] | None:
    if flag:
        return [19]
    return None


def maybe_text(flag: bool) -> str | None:
    if flag:
        return "optional text default"
    return None


def maybe_pair(flag: bool) -> tuple[str, list[int]] | None:
    if flag:
        return ("optional pair default", [20])
    return None


def maybe_flag(flag: bool) -> bool | None:
    if flag:
        return False
    return None


def show_number(value: int | None) -> None:
    if value is None:
        print("missing")
    else:
        print(value)


def basic_maps() -> None:
    huge = 100000000000000000000000000000000000000000000000001
    numbers = {huge: "first", -2: "negative", huge + 0: "replacement", 0: "zero"}
    print(len(numbers))
    print(numbers[huge + 0])
    print(huge + 0 in numbers)
    print(1 not in numbers)
    numbers[-2] = "updated"
    for key, text in numbers.items():
        print(key)
        print(text)
    del numbers[huge + 0]
    numbers[huge] = "reinserted"
    for key in numbers:
        print(key)
    alias = numbers
    alias[0] = "aliased"
    print(numbers[0])

    names = {"é": 1, "e\u0301": 2, "a\0b": 3}
    print(len(names))
    for name in names:  # noqa: PLC0206 - direct key iteration is under test.
        print(name)
        print(names[name])
    print("a\0b" in names)
    print("a" not in names)
    show_number(names.get("é"))
    show_number(names.get("missing"))
    show_number(names.get("missing", None))
    show_number(names.get("missing", maybe_number(True)))
    show_number(names.get("missing", maybe_number(False)))
    print(names.get("missing", 22))


def owned_values() -> None:
    huge = 100000000000000000000000000000000000000000000000003
    texts = {1: "a string whose dictionary entry is replaced"}
    text = texts.get(1, "unused fallback")
    texts[1] = "new"
    print(text)
    missing_text = texts.get(2, None)
    print(missing_text is None)
    missing_text = texts.get(2, maybe_text(True))
    if missing_text is not None:
        print(missing_text)
    missing_text = texts.get(1, maybe_text(False))
    if missing_text is not None:
        print(missing_text)

    rows: dict[str, list[int]] = {"row": [huge]}
    row = rows["row"]
    rows["row"] = []
    row.append(2)
    print(row[0])
    print(row[1])
    print(len(rows["row"]))
    fallback_row = rows.get("missing", [3])
    print(fallback_row[0])
    optional_row = rows.get("missing", maybe_values(True))
    if optional_row is not None:
        print(optional_row[0])
    optional_row = rows.get("missing", maybe_values(False))
    print(optional_row is None)

    pairs: dict[int, tuple[str, list[int]]] = {1: ("kept pair", [huge])}
    pair = pairs.get(1, ("fallback pair", []))
    pairs[1] = ("new pair", [])
    print(pair[0])
    print(pair[1][0])
    optional_pair = pairs.get(9)
    print(optional_pair is None)
    optional_pair = pairs.get(9, maybe_pair(True))
    if optional_pair is not None:
        print(optional_pair[0])
        print(optional_pair[1][0])
    optional_pair = pairs.get(9, maybe_pair(False))
    print(optional_pair is None)

    items = {"item": Item("retained class", huge)}
    item = items.get("item", Item("eager unused class", 0))
    del items["item"]
    print(item.name)
    print(item.number)
    optional_item = items.get("absent", maybe_item(True))
    if optional_item is not None:
        print(optional_item.name)
    optional_item = items.get("absent", maybe_item(False))
    print(optional_item is None)
    items["item"] = Item("optional lookup owner", huge + 1)
    optional_item = items.get("item")
    del items["item"]
    if optional_item is not None:
        print(optional_item.name)
        print(optional_item.number)
    maybe_items: dict[str, Item | None] = {
        "item": Item("stored optional", huge),
        "none": None,
    }
    stored_item = maybe_items.get("item")
    maybe_items["item"] = None
    if stored_item is not None:
        print(stored_item.name)
        print(stored_item.number)
    stored_item = maybe_items.get("none", Item("unused optional fallback", 1))
    print(stored_item is None)

    nested: dict[str, dict[int, int]] = {"inner": {1: huge}}
    inner = nested["inner"]
    nested["inner"] = {}
    print(inner[1])

    flags: dict[int, bool] = {1: False, 2: True}
    print(flags.get(1, True))
    print(flags.get(3, False))
    optional_flag = flags.get(1)
    if optional_flag is not None:
        print(optional_flag)
    optional_flag = flags.get(3, None)
    print(optional_flag is None)
    optional_flag = flags.get(3, maybe_flag(True))
    if optional_flag is not None:
        print(optional_flag)
    optional_flag = flags.get(1, maybe_flag(False))
    if optional_flag is not None:
        print(optional_flag)
    absent: dict[int, None] = {1: None}
    print(absent[1])
    print(absent.get(1))
    print(absent.get(2, None))
    print(1 in absent)
    optional: dict[str, int | None] = {"none": None, "number": huge}
    show_number(optional.get("none"))
    show_number(optional.get("none", 23))
    show_number(optional.get("number", None))
    show_number(optional.get("missing", 23))
    show_number(optional.get("missing", maybe_number(False)))
    optional_flags: dict[int, bool | None] = {1: False, 2: None}
    found_flag = optional_flags.get(1, None)
    if found_flag is not None:
        print(found_flag)
    found_flag = optional_flags.get(2, True)
    print(found_flag is None)


def mark(log: list[int], marker: int, value: int) -> int:
    log.append(marker)
    return value


def marked_map(holder: NumberHolder, log: list[int], marker: int) -> dict[int, int]:
    log.append(marker)
    return holder.values


def replacing_key(holder: NumberHolder, log: list[int]) -> int:
    log.append(1)
    holder.values = {1: 10}
    return 1


def assigning_rhs(holder: NumberHolder, log: list[int]) -> int:
    log.append(1)
    holder.values = {1: 100}
    return 9


def augmented_rhs(holder: NumberHolder, log: list[int]) -> int:
    log.append(3)
    holder.values[1] = 7
    holder.values = {2: 20}
    return 2


def replace_root(holder: ListHolder, log: list[int]) -> list[int]:
    log.append(4)
    holder.values = {"new": [99]}
    return [88]


def replace_value(values: dict[str, list[int]], log: list[int]) -> list[int]:
    log.append(5)
    values["value"] = [7]
    return [8]


def remove_value(values: dict[str, list[int]], log: list[int]) -> list[int]:
    log.append(6)
    del values["value"]
    return [9]


def none_default(log: list[int]) -> None:
    log.append(7)


def evaluation_order() -> None:
    log: list[int] = []
    holder = NumberHolder({0: 0})
    print(replacing_key(holder, log) in marked_map(holder, log, 2))
    for event in log:
        print(event)
    log = []
    marked_map(holder, log, 2)[mark(log, 3, 1)] = assigning_rhs(holder, log)
    print(holder.values[1])
    for event in log:
        print(event)

    # The old BigInt has no owner outside this entry when the RHS replaces it.
    holder.values = {1: 100000000000000000000000000000000000000000000000005}
    original = holder.values
    log = []
    marked_map(holder, log, 1)[mark(log, 2, 1)] += augmented_rhs(holder, log)
    print(original[1])
    print(holder.values[2])
    for event in log:
        print(event)

    # The default removes the only outside root of the captured receiver.
    huge = 100000000000000000000000000000000000000000000000005
    owned = ListHolder({"value": [huge]})
    kept = owned.values.get("value", replace_root(owned, log))
    kept.append(11)
    print(kept[0])
    print(kept[1])
    print(owned.values["new"][0])
    values = {"value": [huge]}
    replaced = values.get("value", replace_value(values, log))
    print(replaced[0])
    removed = values.get("value", remove_value(values, log))
    print(removed[0])
    print(len(values))
    absent = values.get("missing", none_default(log))
    print(absent is None)
    values["value"] = [huge]
    absent = values.get("value", none_default(log))
    if absent is not None:
        print(absent[0])
    nothing: dict[str, None] = {"nothing": None}
    print(nothing.get("nothing", none_default(log)))
    for event in log:
        print(event)


def first_row(values: dict[str, list[int]]) -> list[int]:
    for key, row in values.items():
        del values[key]
        print(key)
        return row
    return []


def changed_start(holder: NumberHolder) -> int:
    holder.values[30] = 300
    holder.values = {999: 9}
    return 100000000000000000000000000000000000000000000000000


def iteration() -> None:
    huge = 100000000000000000000000000000000000000000000000007
    rows = {"first": [huge], "second": [2]}
    for key, row in rows.items():
        if key == "first":
            rows["first"] = []
            rows["second"] = [3]
        print(key)
        print(row[0])
    retained = first_row({"an owned key removed before return": [huge]})
    print(retained[0])
    for key, row in {"temporary": [huge]}.items():
        print(key)
        print(row[0])
    single = {"delete then break": [huge]}
    for key, row in single.items():
        del single[key]
        print(key)
        print(row[0])
        break

    classes = {"class": Item("yielded class", huge)}
    for key, item in classes.items():
        classes[key] = Item("replacement class", 0)
        print(item.name)
        print(item.number)
    pairs = {1: ("yielded tuple", [huge])}
    for number, owned_pair in pairs.items():
        pairs[number] = ("new tuple", [])
        print(owned_pair[0])
        print(owned_pair[1][0])
    optional: dict[int, int | None] = {1: None, 2: 0, 3: huge}
    for number, maybe in optional.items():
        optional[number] = None
        show_number(maybe)

    flags = {1: True, 2: False}
    nothing: dict[str, None] = {"x": None, "y": None}
    for number, flag in flags.items():
        for name, absent in nothing.items():
            print(number)
            print(flag)
            print(name)
            print(absent)
        print(number)
        print(flag)
    for pair in flags.items():
        print(pair[0])
        print(pair[1])
    for index, (number, flag) in enumerate(flags.items(), -huge):
        print(index)
        print(number)
        print(flag)

    # Capture the receiver first, but create its iterator after start runs.
    holder = NumberHolder({10: 100, 20: 200})
    for index, number in enumerate(holder.values, changed_start(holder)):
        print(index)
        print(number)
    print(holder.values[999])
    holder.values = {10: 100, 20: 200}
    for index, (number, value) in enumerate(
        holder.values.items(), changed_start(holder)
    ):
        print(index)
        print(number)
        print(value)
    print(holder.values[999])


basic_maps()
owned_values()
evaluation_order()
iteration()
