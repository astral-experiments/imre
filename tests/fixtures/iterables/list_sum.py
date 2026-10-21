class Holder:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def mutate_and_replace(holder: Holder, events: list[str]) -> int:
    events.append("start")
    holder.values[0] = 1 << 100
    holder.values.extend([-(1 << 100), -7])
    holder.values = [99]
    return 11


def empty(values: list[int], events: list[str]) -> int:
    events.append("empty")
    while len(values) > 0:
        values.pop()
    return 1 << 100


def take_start(values: list[int]) -> int:
    return values.pop()


def make_source(events: list[str]) -> list[int]:
    events.append("source")
    return [1 << 100, -(1 << 100), 7]


def fail_start(events: list[str]) -> int:
    events.append("start")
    raise ValueError("start failed")


def repair(values: list[str], events: list[str]) -> int:
    events.append("repair")
    values[0] = "7"
    values.append("8")
    return 10


huge = 1 << 100
for values in [
    [],
    [0, 1, -1, 2, -3],
    [-(1 << 31), (1 << 31) - 1],
    [-(1 << 31) - 1, 1 << 31],
    [huge, 7, -huge, -9],
    [-(1 << 61), -(1 << 61), 1 << 61, 1 << 61],
]:
    print(sum(values))
    print(sum(values, huge))
    print(sum(values, start=-huge))
    alias = values
    print(sum(alias, -17))

# Capture the original list, then observe every mutation made by start.
holder = Holder([1, 2])
original = holder.values
events: list[str] = []
print(sum(holder.values, mutate_and_replace(holder, events)))
print(len(original))
print(original[0])
print(holder.values[0])
for event in events:
    print(event)

values = [huge, 2, -huge]
alias = values
events = []
print(sum(values, empty(alias, events)))
print(len(values))
for event in events:
    print(event)

# The start can be the last owner of an integer removed from the source.
values = [huge, -huge, 7, huge]
print(sum(values, take_start(values)))
print(len(values))
print(sum(values))

values = [1, 2, 3]
print(sum(values, (values := [huge])[0]))
print(values[0])

for turn in range(12):
    events = []
    try:
        print(sum(make_source(events), fail_start(events)))
    except ValueError as error:
        if turn == 11:
            print(error)
            for event in events:
                print(event)
    print(sum(make_source([]), -7))

# map remains lazy: start repairs the first item before its conversion.
words = ["bad", "2"]
events = []
print(sum(map(int, words), repair(words, events)))
for event in events:
    print(event)
print(len(words))

# Materializing the list first instead raises before start can run.
words = ["bad", "2"]
events = []
try:
    print(sum(list(map(int, words)), repair(words, events)))
except ValueError as error:
    print(error)
print(len(events))
print(words[0])
