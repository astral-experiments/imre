class Number:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


def marked(events: list[str], label: str, value: int) -> int:
    events.append(label)
    return value


def failing(events: list[str], label: str) -> int:
    events.append(label)
    raise ValueError(label)


def text(events: list[str], label: str, value: str) -> str:
    events.append(label)
    return value


def replace(number: Number, events: list[str]) -> int:
    events.append("divisor")
    number.value = 99
    return -7


def reduce(left: int, right: int, initial: int) -> int:
    return sum(divmod(left, right), initial)


huge = 1 << 100
for left in [
    -huge - 1,
    -(1 << 63),
    -(1 << 61),
    -257,
    -256,
    -255,
    -1,
    0,
    1,
    255,
    256,
    257,
    (1 << 61) - 1,
    (1 << 63) - 1,
    huge + 1,
]:
    for right in [-huge, -256, -7, -1, 1, 7, 256, huge]:
        print(sum(divmod(left, right)))
        print(reduce(left, right, huge))
        print(sum(divmod(left, right), start=-huge))

events: list[str] = []
number = Number(huge + 9)
alias = number
print(
    sum(
        divmod(marked(events, "numerator", number.value), replace(alias, events)),
        marked(events, "start", huge),
    )
)
print(number.value)
for event in events:
    print(event)

# divmod is evaluated before the outer start argument, including its errors.
# Supported CPython versions differ in zero-division wording, so check the kind.
for turn in range(12):
    events = []
    try:
        print(
            sum(
                divmod(
                    marked(events, "numerator", huge),
                    marked(events, "zero divisor", 0),
                ),
                failing(events, "must not evaluate start"),
            )
        )
    except ZeroDivisionError:
        if turn == 11:
            print("zero division")
            for event in events:
                print(event)

for fail_left in [True, False]:
    events = []
    try:
        if fail_left:
            print(
                sum(
                    divmod(
                        failing(events, "numerator"),
                        marked(events, "must not evaluate divisor", 3),
                    ),
                    marked(events, "must not evaluate start", 4),
                )
            )
        else:
            print(
                sum(
                    divmod(
                        marked(events, "numerator", huge),
                        failing(events, "divisor"),
                    ),
                    marked(events, "must not evaluate start", 4),
                )
            )
    except ValueError as error:
        print(error)
    for event in events:
        print(event)

# Start may throw after the BigInt quotient and remainder are produced.
for turn in range(24):
    events = []
    try:
        print(sum(divmod(-huge - 3, 7), failing(events, "start failure")))
    except ValueError as error:
        if turn == 23:
            print(error)
            print(len(events))
    print(sum(divmod(-huge - 3, -7), huge))

captured = huge
print(sum(divmod(captured, 7), start=(captured := -huge)))
print(captured)

# The optimization also applies to fixed tuples. Elements are evaluated before
# start, while map conversion happens after it.
events = []
print(
    sum(
        (marked(events, "left", huge), marked(events, "right", -huge + 2)),
        start=marked(events, "start", 3),
    )
)
for event in events:
    print(event)
events = []
try:
    print(
        sum(
            map(int, (text(events, "left", "2"), text(events, "right", "x"))),
            start=marked(events, "start", 3),
        )
    )
except ValueError as error:
    print(error)
for event in events:
    print(event)
events = []
try:
    print(
        sum(
            map(int, (text(events, "left", "x"), text(events, "right", "2"))),
            start=failing(events, "start failure"),
        )
    )
except ValueError as error:
    print(error)
for event in events:
    print(event)
events = []
print(sum((), start=marked(events, "empty start", huge)))
print(len(events))

# A separately retained tuple stays valid when start rebinds its original name.
pair = (huge, -huge + 7)
saved_pair = pair
print(sum(pair, start=(pair := (11, 12))[0]))
print(saved_pair[0])
print(pair[0])
