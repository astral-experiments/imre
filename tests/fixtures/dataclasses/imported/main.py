from facade import Entry
from geometry import Point, make
from decorators import record
from models import Stamp as ModelStamp, erased, make_stamp


@record(slots=True)
class Stamp:
    value: int


def inspect(stamp: object, original: ModelStamp) -> None:
    print(isinstance(stamp, ModelStamp))
    print(isinstance(stamp, Stamp))
    print(stamp is original)
    print(stamp == original)
    print(stamp == Stamp(3))
    if isinstance(stamp, ModelStamp):
        print(stamp.value)


def consume(rows: list[Point]) -> Point:
    return rows.pop()


row = Entry.named("aliased")
row.values.append(4)
print(row.name)
print(row.count)
print(row)

local = ModelStamp(3)
inspect(erased(local), local)
inspect(make_stamp(3), local)
inspect(Stamp(3), local)

rows = make(3.0)
saved = consume(rows)
rows.append(Point(4.0, 5.0))
print(saved.scaled_x(2.0))
print(Point(3.0, 2.0) == saved)
print(rows[0])
