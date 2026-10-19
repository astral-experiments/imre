# ruff: noqa: SIM222
# Literal operands exercise contextual types for empty containers.


def show_optional(value: list[int] | None) -> None:
    print(bool(value))
    if value:
        print(value[0])
    if value is None:
        print("none")
    else:
        print(len(value))


values: list[int] = []
print(bool(values))
print(not values)
values.append(7)
print(bool(values))
selected = values or [9]
selected.append(8)
print(len(values))
print(values[1])
selected = values and [11]
selected.append(12)
print(len(values))
print(selected[1])
empty: list[int] = []
selected = empty and [13]
selected.append(14)
print(empty[0])

words: list[str] = [] or ["word"]
print(words[0])
mapping: dict[str, int] = {} or {"item": 5}
print(mapping["item"])
unique: set[int] = set() or {4}
print(4 in unique)
print(bool(mapping))
print(bool(unique))
del mapping["item"]
unique.discard(4)
print(bool(mapping))
print(bool(unique))
print(not mapping)
print(not unique)
objects: list[list[str]] = []
print(bool(objects))
objects.append([])
print(bool(objects))
print(bool(objects[0]))
show_optional(None)
show_optional([])
show_optional([31])

pending = [1, 2, 3]
while pending:
    print(pending.pop())
print(bool(pending))
