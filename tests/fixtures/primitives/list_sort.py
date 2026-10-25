from typing import Callable


class Inspect:
    values: list[str]
    actions: int

    def __init__(self, values: list[str], actions: int) -> None:
        self.values = values
        self.actions = actions

    def glob(self, pattern: str) -> list[str]:
        return self.values

    def key(self, item: str) -> tuple[int, str]:
        print(f"key {item} visible={','.join(self.values)}")
        if item == "b1":
            if self.actions == 1 or self.actions == 3:
                self.values.append("temporary")
            elif self.actions == 2:
                for extra in ["a", "b", "c", "d"]:
                    self.values.append(extra)
                for index in range(4):
                    self.values.pop()
            elif self.actions == 4:
                self.values.sort(key=lambda value: value)
                self.values.clear()
                self.values.extend([])
                self.values.extend(reversed([]))
                self.values.extend(self.glob("empty"))
            elif self.actions == 5:
                self.values.extend(s for s in ["x"] if s == "")
            elif self.actions == 6:
                self.values.pop()
        if item == "a2" and self.actions == 3:
            raise KeyError("key failed")
        return (len(item), item[0])


def keywords(values: list[str], key: Callable[[str], tuple[int, str]]) -> Callable[[str], tuple[int, str]]:
    print(f"key argument {','.join(values)}")
    values.append("c1")
    return key


def reverse_arg(values: list[str]) -> bool:
    print(f"reverse argument {','.join(values)}")
    return True


def observe(action: int) -> None:
    values = ["b1", "a2", "b3", "a4"]
    alias = values
    key = Inspect(alias, action)
    try:
        result = values.sort(reverse=reverse_arg(alias), key=keywords(alias, key.key))
        print(result is None)
    except ValueError as error:
        print(str(error))
    except KeyError:
        print("key error")
    except IndexError:
        print("empty pop")
    print(f"after {','.join(alias)}")


class IntKey:
    items: list[int]

    def __init__(self, items: list[int]) -> None:
        self.items = items

    def changed(self, value: int) -> int:
        if value == 3:
            self.items.append(1267650600228229401496703205376)
            self.items.pop()
            self.items.sort()
        return value

    def unhinted(self, value: int) -> int:
        self.items.extend(n for n in range(0))
        self.items.clear()
        return value


class Row:
    rank: int
    label: str

    def __init__(self, rank: int, label: str) -> None:
        self.rank = rank
        self.label = label


def row_key(row: Row) -> int:
    row.label = row.label + "!"
    return row.rank


def bad_key(rows: list[Row]) -> Callable[[Row], int]:
    rows.append(Row(0, "added"))
    raise ValueError("factory failed")


for mode in range(7):
    observe(mode)

numbers = [3, -1267650600228229401496703205376, 1]
numbers_alias = numbers
int_key = IntKey(numbers_alias)
try:
    numbers.sort(key=int_key.changed)
except ValueError as error:
    print(str(error))
try:
    numbers.sort(reverse=True, key=int_key.unhinted)
except ValueError as error:
    print(str(error))
for number in numbers_alias:
    print(number)

pairs = [(1, "bbb"), (1, "aaa"), (0, "ccc")]
pairs.sort(key=None)
for n, text in pairs:
    print(f"{n} {text}")
empty: list[str] = []
empty.sort(key=lambda item: item)
single = ["alone"]
single.sort(key=Inspect(single, 0).key)
print(f"small {len(empty)} {single[0]}")


rows = [Row(2, "first"), Row(1, "second"), Row(2, "third")]
owned_first = rows[0]
try:
    rows.sort(key=bad_key(rows))
except ValueError as error:
    print(str(error))
print(f"before {rows[0].label} {rows[-1].label}")
rows.sort(key=row_key)
print(f"shared {owned_first.label}")
for row in rows:
    print(row.label)


helper = Inspect(["one", "two"], 0)
helper.values.extend(helper.glob("alias"))
print(",".join(helper.values))
