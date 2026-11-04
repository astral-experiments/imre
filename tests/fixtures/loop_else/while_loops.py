# ruff: noqa: PLW0120
# Else suites explicitly exercise normal condition failure.


def countdown(value: int, break_at: int) -> None:
    while value > 0:
        if value == break_at:
            break
        value -= 1
        if value == 1:
            continue
        print(value)
    else:
        print("while exhausted")
    print(value)


def condition(events: list[int], limit: int) -> bool:
    events.append(len(events))
    return len(events) <= limit


def until_nonnegative(values: list[int]) -> int:
    while True:
        value = values.pop()
        if value >= 0:
            return value


def inferred():
    while 1:
        for value in range(2):
            if value:
                break
        return "ready"


def assigned_on_break(value: int) -> int:
    while True:
        result = value + 1
        break
    return result


def inner_else() -> int:
    while True:
        while 1:
            return 7
        else:
            break


countdown(0, -1)
countdown(4, -1)
countdown(4, 2)
events: list[int] = []
while condition(events, 2):
    print(len(events))
else:
    print("condition failed normally")
print(len(events))
while True:
    print("breaking")
    break
else:
    print("unreachable")
pending = [1, 2]
while pending:
    print(pending.pop())
else:
    print(len(pending))
print(until_nonnegative([3, -1]))
print(inferred().upper())
print(assigned_on_break(4))
print(inner_else())
