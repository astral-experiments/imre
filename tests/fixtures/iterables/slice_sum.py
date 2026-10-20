class Holder:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def source(holder: Holder, events: list[str], fail: bool) -> list[int]:
    events.append("source")
    if fail:
        raise ValueError("source failure")
    return holder.values


def bound(events: list[str], label: str, value: int, fail: bool) -> int:
    events.append(label)
    if fail:
        raise ValueError(label)
    return value


def mutate_start(holder: Holder, events: list[str]) -> int:
    events.append("start")
    holder.values[0] = 99
    while len(holder.values) > 0:
        holder.values.pop()
    holder.values.extend([7, 8])
    holder.values = [55]
    return 1 << 100


def fail_start(events: list[str]) -> int:
    events.append("start")
    raise ValueError("start failure")


def changing_bound(holder: Holder, events: list[str]) -> int:
    events.append("changing bound")
    holder.values[0] = 77
    holder.values = [99]
    return 0


def sliced(
    values: list[int],
    first: int | None,
    last: int | None,
    stride: int | None,
    initial: int,
) -> int:
    return sum(values[first:last:stride], initial)


huge = 1 << 100
values = [huge, 3, -huge, -7, 11, -13]
cases: list[tuple[int | None, int | None, int | None]] = [
    (None, None, None),
    (None, None, -1),
    (None, -1, -1),
    (1, 5, 2),
    (5, 0, -2),
    (4, 1, 1),
    (-huge, huge, 1),
    (huge, -huge, -1),
    (None, None, huge),
    (None, None, -huge),
    (huge, huge, huge),
]
for first, last, stride in cases:
    print(sliced(values, first, last, stride, 0))
    print(sliced(values, first, last, stride, huge))
    print(sliced([], first, last, stride, -huge))

# The slice owns its selected values before the outer start can mutate, empty,
# or replace the source. The subtotal must preserve that snapshot too.
holder = Holder([huge, 3, -huge, -7, 11])
original = holder.values
events: list[str] = []
print(
    sum(
        source(holder, events, False)[
            bound(events, "first", 0, False) : bound(events, "last", 5, False) : bound(
                events, "stride", 2, False
            )
        ],
        start=mutate_start(holder, events),
    )
)
print(len(original))
print(original[0])
print(holder.values[0])
for event in events:
    print(event)

# Source capture precedes the bounds; changes made by the bounds are visible
# when the slice is made, even if the original source name was rebound.
holder = Holder([1, 2, 3])
original = holder.values
events = []
print(
    sum(
        source(holder, events, False)[changing_bound(holder, events) : 2],
        mutate_start(Holder(original), events),
    )
)
print(holder.values[0])
print(original[0])
for event in events:
    print(event)

# Source/bounds failures and slice normalization precede outer-start evaluation.
for failure in range(5):
    holder = Holder([huge, 2, -huge])
    events = []
    try:
        print(
            sum(
                source(holder, events, failure == 0)[
                    bound(events, "first", 0, failure == 1) : bound(
                        events, "last", 3, failure == 2
                    ) : bound(events, "stride", 0, failure == 3)
                ],
                fail_start(events),
            )
        )
    except ValueError as error:
        print(error)
    for event in events:
        print(event)

# A zero stride is still an error for an empty source and omitted bounds.
for values in [[], [huge]]:
    events = []
    step = 0
    try:
        print(sum(values[::step], fail_start(events)))
    except ValueError as error:
        print(error)
    print(len(events))

# Release selected BigInts and their subtotal when a later start raises.
for turn in range(16):
    events = []
    try:
        print(sum([huge, 3, -huge, 7][::-2], fail_start(events)))
    except ValueError as error:
        if turn == 15:
            print(error)
            print(len(events))
    print(sum([huge, -huge, -7][:], start=huge))

values = [1, 2, 3]
print(sum(values[::-1], (values := [huge])[0]))
print(values[0])
