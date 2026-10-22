from dataclasses import dataclass
from enum import StrEnum


class Side(StrEnum):
    OLD = "old"
    NEW = "new"


@dataclass
class Data:
    old: list[int]
    new: list[int]
    total: int

    def choose(self, *, side: Side) -> list[int]:
        return self.old if side == Side.OLD else self.new

    def flagged(self, flag: bool) -> list[int]:
        return self.new if flag else self.old

    def replace(self) -> Side:
        self.old = [17]
        return Side.OLD

    def fail(self) -> int:
        self.total = 19
        raise ValueError("replacement")


def length(data: Data, side: Side) -> int:
    selected = data.choose(side=side)
    return len(selected) + len(data.flagged(False))


def snapshot(data: Data) -> int:
    selected = data.choose(side=Side.OLD)
    data.replace()
    return selected[0]


def main() -> None:
    big = 1 << 100
    data = Data([big, -big], [3], big)
    print(length(data, Side.OLD))
    print(length(data, Side.NEW))
    print(snapshot(data))
    alias = data.choose(side=Side.NEW)
    data.new = [9]
    print(alias[0])
    print(data.choose(side=data.replace())[0])
    data.total += len(data.choose(side=Side.OLD))
    print(data.total)
    try:
        data.total += data.fail()
    except ValueError as error:
        print(str(error))
        print(data.total)
    data.total = (1 << 62) - 1
    data.total += 1
    print(data.total)
    data.total -= data.total
    print(data.total)


main()
