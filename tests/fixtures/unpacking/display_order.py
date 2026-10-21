# ruff: noqa: PIE800
# Nested dictionary displays exercise evaluation through an expansion boundary.


def change_list(values: list[int]) -> int:
    values[0] = 9
    values.append(3)
    print("change list")
    return 4


def change_dict(values: dict[str, int]) -> int:
    values["first"] = 9
    values["added"] = 3
    print("change dictionary")
    return 4


def change_set(values: set[int]) -> int:
    values.discard(1)
    values.add(3)
    print("change set")
    return 4


def note(log: list[int], event: int) -> int:
    log.append(event)
    return event


def fail() -> int:
    print("later element failed")
    raise ValueError("element")


values = [1, 2]
expanded = [*values, change_list(values), *values]
for value in expanded:
    print(value)
print(len(values))

mapping = {"first": 1, "second": 2}
merged = {**mapping, "middle": change_dict(mapping), **mapping}
for key, value in merged.items():
    print(key)
    print(value)

source = {1, 2}
members = {*source, change_set(source), *source}
print(len(members))
print(1 in members)
print(1 in source)
print(3 in members)
print(4 in members)

events: list[int] = []
ordered = {note(events, 1): note(events, 2), **{note(events, 3): note(events, 4)}}
for event in events:
    print(event)
for number, value in ordered.items():
    print(number)
    print(value)

try:
    discarded = [*values, change_list(values), fail()]
except ValueError:
    print(len(values))
try:
    discarded_mapping = {**mapping, "changed": change_dict(mapping), "failed": fail()}
except ValueError:
    print(mapping["first"])
    print(mapping["added"])
