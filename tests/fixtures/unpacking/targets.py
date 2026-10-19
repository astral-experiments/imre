# ruff: noqa: PLW0128
# Repeated assignment targets must retain their left-to-right store order.


class Box:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


def shrink(values: list[int]) -> int:
    print("shrink target receiver")
    values.pop()
    return 0


def alter(values: list[list[int]]) -> int:
    values[1].append(99)
    return 0


box = Box(0)
values = [0, 0]
position = 0
position, values[position] = [1, 12]
print(position)
print(values[0])
print(values[1])
box.value, values[0] = [21, 22]
print(box.value)
print(values[0])

snapshot = [31, 32]
snapshot[shrink(snapshot)], captured = snapshot
print(len(snapshot))
print(snapshot[0])
print(captured)

rows = [[41], [42]]
rows[alter(rows)], (nested_first, nested_second) = rows
print(rows[0][0])
print(nested_first)
print(nested_second)

same = 0
same, same = [51, 52]
print(same)

swapped = [[61], [62]]
swapped[0], swapped[1] = swapped[1], swapped[0]
print(swapped[0][0])
print(swapped[1][0])

for box.value, values[0] in [[71, 72], [73, 74]]:
    print(box.value)
    print(values[0])

observed = [box.value + values[1] for box.value, values[1] in [[81, 82], [83, 84]]]
for total in observed:
    print(total)
print(box.value)
print(values[1])

offset: int | None = None
slots = [0, 0, 0]
for slots[(offset := 0 if offset is None else offset + 1)] in [91, 92, 93]:
    pass
print(slots[0])
print(slots[1])
print(slots[2])
