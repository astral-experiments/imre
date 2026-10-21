class Box:
    values: list[int]

    def __init__(self) -> None:
        self.values = []


def make(events: list[int]) -> list[int]:
    events.append(1)
    return [10]


def receiver(box: Box, events: list[int]) -> Box:
    events.append(2)
    return box


def index(events: list[int]) -> int:
    events.append(3)
    return 0


def fail() -> int:
    print("rhs failed")
    raise ValueError("rhs")


events: list[int] = []
box = Box()
slots: list[list[int]] = [[]]
first = receiver(box, events).values = slots[index(events)] = make(events)
first.append(20)
print(len(box.values))
print(slots[0][1])
for event in events:
    print(event)

left = right = [30]
left.append(40)
print(right[1])
left = [50]
print(right[0])

texts: list[str] = ["old"]
new = texts = []
new.append("shared empty")
print(texts[0])

maybe: int | None = None
other: int | None = None
maybe = other = 7
print(maybe + other)
maybe = other = None
print(maybe is None)
print(other is None)

number = 90
another = 91
try:
    number = another = fail()
except ValueError:
    print(number)
    print(another)

untouched = 0
try:
    assigned = slots[8] = make(events)
    untouched = 1
except IndexError:
    print("later target failed")
print(untouched)
print(len(events))

# Each target group extracts from the shared RHS when its turn is reached.
source = [101, 102]
source[1], saved = other_first, other_second = source
print(saved)
print(other_first)
print(other_second)
print(source[1])

whole = head, *tail = [201, 202]
tail.append(203)
print(len(whole))
print(head)
print(tail[1])
