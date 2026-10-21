"""Typed slots and partial allocations remain safe on every exceptional edge."""


def fail() -> int:
    raise ValueError("partial")


class Item:
    number: int

    def __init__(self, number: int) -> None:
        self.number = number


class Partial:
    flag: bool | None
    nothing: None
    number: int | None
    text: str | None
    row: list[int] | None
    mapping: dict[str, bool | None]
    pair: tuple[int | None, Item | None]
    last: int
    unwritten_item: Item
    unwritten_text: str
    unwritten_row: list[int]

    def __init__(self) -> None:
        self.flag = False
        self.nothing = None
        self.number = 1000000000000000000000000000001
        self.text = "owned" + " text"
        self.row = [1000000000000000000000000000002]
        self.mapping = {"false": False, "absent": None}
        self.pair = (1000000000000000000000000000003, Item(4))
        self.last = fail()
        self.unwritten_item = Item(0)
        self.unwritten_text = "never initialized"
        self.unwritten_row = [0]


def partial_tuple() -> tuple[bool | None, list[int] | None, str | None, int]:
    return (False, [1000000000000000000000000000004], "", fail())


def partial_list() -> list[int | None]:
    return [None, 1000000000000000000000000000005, fail()]


def partial_mapping() -> dict[str, int | None]:
    return {"absent": None, "owned": 1000000000000000000000000000006, "bad": fail()}


def return_flag() -> bool | None:
    try:
        return False
    finally:
        print("preserve false")


def discard_return() -> int | None:
    try:
        try:
            return 1000000000000000000000000000007
        finally:
            raise ValueError("discard integer return")
    except ValueError:
        return None


def return_owner() -> list[int] | None:
    values: list[list[int] | None] = [[1000000000000000000000000000008]]
    try:
        return values.pop()
    finally:
        values.append(None)


def replaced_by_error() -> Item | None:
    try:
        return Item(1000000000000000000000000000009)
    finally:
        fail()


def constructions() -> None:
    for turn in range(32):
        try:
            Partial()
        except ValueError:
            pass
        try:
            partial_tuple()
        except ValueError:
            pass
        try:
            partial_list()
        except ValueError:
            pass
        try:
            partial_mapping()
        except ValueError:
            pass
        try:
            replaced_by_error()
        except ValueError:
            pass
    print("partial cleanup complete")
    flag = return_flag()
    if flag is None:
        print("wrong absent")
    else:
        print(flag)
    print(discard_return() is None)
    row = return_owner()
    if row is not None:
        print(row[0])


def iterators() -> None:
    saved: Item | None = None
    values: dict[str, Item | None] = {
        "first": Item(1000000000000000000000000000010),
        "second": None,
    }
    alias = values
    try:
        for key, value in values.items():
            saved = value
            del alias[key]
    except RuntimeError:
        print("size changed")
    if saved is not None:
        print(saved.number)
    values["first"] = None
    values["third"] = Item(11)
    for key, value in values.items():
        values[key] = None
        print(key)
        if value is None:
            print("absent item")
        else:
            print(value.number)
    flags: list[bool | None] = [False, None, True]
    try:
        while len(flags) > 0:
            flag = flags.pop()
            print(flag is None)
            if flag is not None:
                print(flag)
        flags.pop()
    except IndexError:
        print("empty optional pop")


constructions()
iterators()
