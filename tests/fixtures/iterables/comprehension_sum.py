def visit(value: int, values: list[int], events: list[str]) -> int:
    events.append(f"body {value}")
    if value == 1:
        values.append(4)
    if value == 2:
        values[2] = 30
    return value * value


def select(value: int, events: list[str]) -> bool:
    events.append(f"filter {value}")
    return value != 2


def start(values: list[int], events: list[str]) -> int:
    events.append("start")
    values[0] = 1000
    values.append(1000)
    return -(1 << 100)


def fail(value: int, events: list[str]) -> int:
    events.append(f"body {value}")
    if value == 3:
        raise ValueError("body failed")
    return value


def fail_start(events: list[str]) -> int:
    events.append("start failed")
    raise ValueError("start failed")


huge = 1 << 100
for values in [[], [1, 2, -3], [huge, -huge, 7], [-huge, 1, huge]]:
    print(sum([value for value in values]))
    print(sum([value * 2 for value in values], huge))
    print(sum([value + 1 for value in values], start=-huge))
    print(sum([left * right for left in values for right in [-2, 0, 3] if right]))

# The eager producer sees live-list mutation, and the later start sees its effects.
values = [1, 2, 3]
events: list[str] = []
print(sum([visit(value, values, events) for value in values], start(values, events)))
for event in events:
    print(event)
for value in values:
    print(value)

events = []
print(sum([value * 3 for value in [1, 2, 3] if select(value, events)], 7))
for event in events:
    print(event)

# An eager producer error prevents start, while a start error follows every body.
for turn in range(12):
    events = []
    try:
        print(sum([fail(value, events) for value in [1, 2, 3, 4]], fail_start(events)))
    except ValueError as error:
        if turn == 11:
            print(error)
            for event in events:
                print(event)
    events = []
    try:
        print(sum([fail(value, events) for value in [1, 2]], fail_start(events)))
    except ValueError as error:
        if turn == 11:
            print(error)
            for event in events:
                print(event)

# Assignment expressions export their writes even when a later iteration raises.
last = 99
print(sum([(last := value) for value in [3, 4, 5]], last))
print(last)
try:
    print(sum([(last := 10 // value) for value in [2, 1, 0]]))
except ZeroDivisionError:
    print(last)

values = [1, 2, 3]
print(sum([value for value in values], (values := [huge])[0]))
print(values[0])
