from facade import Entry
from geometry import Point, make


def consume(rows: list[Point]) -> Point:
    return rows.pop()


row = Entry.named("aliased")
row.values.append(4)
print(row.name)
print(row.count)
print(row)

rows = make(3.0)
saved = consume(rows)
rows.append(Point(4.0, 5.0))
print(saved.scaled_x(2.0))
print(Point(3.0, 2.0) == saved)
print(rows[0])
