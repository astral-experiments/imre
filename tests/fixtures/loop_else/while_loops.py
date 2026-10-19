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
