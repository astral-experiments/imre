from dataclasses import dataclass
from enum import StrEnum


class Rank(StrEnum):
    LAST = "z"
    FIRST = "a"
    EMPTY = ""
    ALSO_FIRST = FIRST


@dataclass
class Row:
    rank: Rank
    previous: Rank | None


row = Row(Rank.LAST, None)
print(row)
row.previous = Rank.ALSO_FIRST
print(row)
print(row == Row(Rank.LAST, Rank.FIRST))
values: list[Rank] = [row.rank, Rank.EMPTY, Rank.FIRST]
print(repr(values))
for rank in sorted(values):
    print(rank.name)
lookup: dict[Rank, int] = {Rank.ALSO_FIRST: 4, Rank.LAST: 5}
print(lookup[Rank.FIRST])
print(repr(lookup))
keys: set[Rank] = {Rank.FIRST, Rank.ALSO_FIRST}
print(len(keys))
print(Rank.FIRST in keys)
indexed: dict[tuple[Rank, int], int] = {(Rank.FIRST, 8): 9}
print(indexed[(Rank.ALSO_FIRST, 8)])
try:
    print(lookup[Rank.EMPTY])
except KeyError as error:
    print(str(error))
