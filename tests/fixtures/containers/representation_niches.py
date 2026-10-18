"""Absent values stay distinct from valid zero words across typed storage."""


class Item:
    number: int

    def __init__(self, number: int) -> None:
        self.number = number


class Record:
    before: bool
    nothing: None
    number: int | None
    flag: bool | None
    text: str | None
    item: Item | None
    after: bool

    def __init__(
        self, number: int | None, flag: bool | None, text: str | None, item: Item | None
    ) -> None:
        self.before = False
        self.nothing = None
        self.number = number
        self.flag = flag
        self.text = text
        self.item = item
        self.after = True


def show_number(value: int | None) -> None:
    if value is None:
        print("absent number")
    else:
        print(value)


def show_flag(value: bool | None) -> None:
    if value is None:
        print("absent flag")
    else:
        print(value)


def show_text(value: str | None) -> None:
    if value is None:
        print("absent text")
    else:
        print(len(value))
        print(value)


def returned_number(value: int | None) -> int | None:
    return value


def returned_flag(value: bool | None) -> bool | None:
    return value


def capture(first: int | None, ignored: None) -> int | None:
    return first


def clear_number(record: Record) -> None:
    record.number = None


def flag_default(values: dict[str, bool | None], log: list[int]) -> bool | None:
    log.append(1)
    values["present"] = None
    return False


def scalar_slots() -> None:
    numbers: list[int | None] = [
        None,
        0,
        -1,
        4611686018427387903,
        4611686018427387904,
        -4611686018427387904,
        -4611686018427387905,
        1000000000000000000000000000000,
        -1000000000000000000000000000000,
    ]
    for value in numbers:
        show_number(returned_number(value))
    while len(numbers) > 0:
        show_number(numbers.pop())
    flags: list[bool | None] = [None, False, True]
    for flag in flags:
        show_flag(returned_flag(flag))
    flags[0] = False
    flags[1] = None
    for flag in flags:
        show_flag(flag)
    texts: list[str | None] = [None, "", "owned" + " text"]
    while len(texts) > 0:
        show_text(texts.pop())
    nones: list[None] = [None, None]
    print(nones.pop())
    print(nones[0])


def fields_and_defaults() -> None:
    record = Record(1000000000000000000000000000000, False, "", Item(7))
    alias = record
    snapshot = capture(record.number, clear_number(alias))
    show_number(snapshot)
    print(record.number is None)
    print(record.before)
    print(record.nothing)
    print(record.after)
    show_flag(record.flag)
    show_text(record.text)
    saved_item = record.item
    alias.item = None
    if saved_item is not None:
        print(saved_item.number)
    record.flag = None
    record.number = 0
    record.text = None
    show_flag(alias.flag)
    show_number(alias.number)
    show_text(alias.text)
    record.flag = True
    show_flag(alias.flag)

    flags: dict[str, bool | None] = {"present": False, "absent": None}
    log: list[int] = []
    show_flag(flags.get("present", flag_default(flags, log)))
    show_flag(flags.get("missing", False))
    flags["present"] = False
    show_flag(flags.get("present"))
    show_flag(flags.get("absent", True))
    show_flag(flags.get("missing"))
    for key, value in flags.items():
        flags[key] = True
        print(key)
        show_flag(value)
    print(len(log))
    numbers: dict[int, int | None] = {0: 0, 1: None, 2: 4611686018427387904}
    for number_key, number in numbers.items():
        numbers[number_key] = None
        show_number(number)
    show_number(numbers.get(99, 0))
    show_number(numbers.get(0, 1))


def nested_owners() -> None:
    pairs: list[tuple[bool | None, int | None, str | None]] = [
        (False, 1000000000000000000000000000001, ""),
        (None, 0, None),
    ]
    saved = pairs[0]
    pairs[0] = (True, None, "replacement")
    show_flag(saved[0])
    show_number(saved[1])
    show_text(saved[2])
    nested: list[list[bool | None] | None] = [None, [False, None, True]]
    row = nested.pop()
    nested[0] = []
    if row is not None:
        for flag in row:
            show_flag(flag)
    mappings: dict[str, dict[int, int | None] | None] = {
        "owned": {0: 1000000000000000000000000000002, 1: None},
        "absent": None,
    }
    mapping = mappings.get("owned")
    del mappings["owned"]
    if mapping is not None:
        show_number(mapping[0])
        show_number(mapping[1])
        mapping[0] = 0
        show_number(mapping[0])
    sets: list[set[int] | None] = [None, {1000000000000000000000000000003}]
    kept = sets[1]
    sets[1] = None
    if kept is not None:
        print(1000000000000000000000000000003 in kept)
        kept.discard(1000000000000000000000000000003)
        print(len(kept))


scalar_slots()
fields_and_defaults()
nested_owners()
