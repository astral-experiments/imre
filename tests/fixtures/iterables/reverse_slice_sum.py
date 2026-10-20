class Holder:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def take(holder: Holder, events: list[str]) -> list[int]:
    events.append("source")
    captured = holder.values
    holder.values = [99]
    return captured


def start(values: list[int], events: list[str]) -> int:
    events.append("start")
    while len(values) > 0:
        values.pop()
    return 7


def visit(value: int, values: list[int]) -> int:
    values[0] = 1 << 100
    values.append(9)
    return value


def select(value: int, values: list[int]) -> bool:
    values[0] = 1 << 100
    return value != 2


huge = 1 << 100
samples: list[list[int]] = [
    [],
    [0],
    [1, 2],
    [1, 2, 3, 4, 5, 6, 7],
    [huge, -7, -huge, 9],
]
for values in samples:
    # Literal bounds exercise each supported reverse cursor shape.
    print(sum([value * 7 + 1 for value in values[::-1]]))
    print(sum([value * 7 + 1 for value in values[-1::-2]]))
    print(sum([value * 7 + 1 for value in values[-2::-2]]))
    print(sum([value * 7 + 1 for value in values[-8::-3]]))
    print(sum([value * 7 + 1 for value in values[-4611686018427387904::-1]]))
    print(sum([value * 7 + 1 for value in values[::-4611686018427387904]]))
    print(sum([value for value in values[-1::-2] if value > 0 if value % 3]))
    # Clipped positive bounds, explicit stops, and BigInt bounds keep slicing.
    print(sum([value for value in values[100::-2]]))
    print(sum([value for value in values[-1:-4:-1]]))
    print(sum([value for value in values[-4611686018427387905::-1]]))

# The first failing selected element is observable even without callbacks.
for values in [[0, -1], [-1, 0], [0, 1, -1], [-1, 1, 0]]:
    try:
        print(sum([10 // value + (1 << value) for value in values[::-1]]))
    except ZeroDivisionError:
        print("division failed")
    except ValueError:
        print("shift failed")

# Evaluate the producer once before local captures; finish the reduction before
# the outer start mutates the captured source.
holder = Holder([1, 2, 3])
original = holder.values
events: list[str] = []
print(
    sum(
        [value + holder.values[0] for value in take(holder, events)[::-1]],
        start(original, events),
    )
)
print(len(original))
print(holder.values[0])
for event in events:
    print(event)

# Effectful bodies and filters must retain the private slice snapshot.
values = [1, 2, 3]
print(sum([visit(value, values) for value in values[::-1]]))
print(len(values))
print(values[0])
values = [1, 2, 3]
print(sum([value for value in values[::-1] if select(value, values)]))
print(values[0])

events = []
values = [1, 2]
try:
    print(sum([value for value in values[::-0]], start(values, events)))
except ValueError as error:
    print(error)
print(len(events))
print(len(values))
