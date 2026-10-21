def view(value: bytes, start: int | None, stop: int | None, step: int | None) -> bytes:
    return value[start:stop:step]


def source(events: list[int]) -> bytes:
    events.append(0)
    return b"\0a\x80b\xff"


def bound(events: list[int], value: int) -> int:
    events.append(value)
    return value


data = b"\0a\x80b\xff"
bounds: list[int | None] = [
    None,
    -1000000000000000000000000000000,
    -7,
    -1,
    0,
    1,
    7,
    1000000000000000000000000000000,
]
steps: list[int | None] = [
    None,
    -1000000000000000000000000000000,
    -3,
    -1,
    1,
    3,
    1000000000000000000000000000000,
]
for start in bounds:
    for stop in bounds:
        for step in steps:
            print(view(data, start, stop, step))
            print(view(b"", start, stop, step))
print(data[::-1])
print(data[:-1:-1])
print(data[:])
print(data[None:None:None])
events: list[int] = []
print(source(events)[bound(events, 1) : bound(events, 5) : bound(events, 2)])
for event in events:
    print(event)
try:
    print(source(events)[bound(events, 1) : bound(events, 5) : bound(events, 0)])
except ValueError as error:
    print(error)
for event in events:
    print(event)
