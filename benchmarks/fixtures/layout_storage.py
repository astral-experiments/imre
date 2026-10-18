class Leaf:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


class Record:
    value: int
    flag: bool
    name: str
    leaf: Leaf

    def __init__(self, value: int, flag: bool, name: str, leaf: Leaf) -> None:
        self.value = value
        self.flag = flag
        self.name = name
        self.leaf = leaf


def optionals(size: int) -> int:
    numbers: list[int | None] = [None]
    leaves: list[Leaf | None] = [None]
    for index in range(size):
        numbers.append(index)
        leaves.append(Leaf(index))
    numbers.append(None)
    leaves.append(None)
    total = 0
    for index in range(len(numbers)):
        number = numbers[index]
        leaf = leaves[index]
        if number is not None:
            total += number
        if leaf is not None:
            total += leaf.value
    return total


def records(size: int) -> int:
    values: list[Record] = []
    name = "fixed"
    for index in range(size):
        values.append(Record(index, index % 2 == 0, name, Leaf(index)))
    total = 0
    for value in values:
        total += value.value + value.leaf.value
        if value.flag:
            total += 1
        total += len(value.name)
    return total


def sequences(size: int) -> int:
    flags: list[bool] = []
    names: list[str] = []
    pairs: list[tuple[int, str]] = []
    name = "fixed"
    pair_name = "pair"
    for index in range(size):
        flags.append(index % 2 == 0)
        names.append(name)
        pairs.append((index, pair_name))
    total = 0
    for index in range(size):
        if flags[index]:
            total += 1
        total += len(names[index])
        pair = pairs[index]
        total += pair[0] + len(pair[1])
    return total


mode = int(input())
size = int(input())
assert size >= 0
if mode == 0:
    print(optionals(size))
elif mode == 1:
    print(records(size))
else:
    assert mode == 2
    print(sequences(size))
