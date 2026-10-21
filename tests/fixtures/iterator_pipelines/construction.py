def source(log: list[str]) -> list[int]:
    log.append("source")
    return [1, 2]


def item(log: list[str], value: int) -> int:
    log.append("item " + str(value))
    return value


def keep(log: list[str], value: int) -> bool:
    log.append("filter " + str(value))
    return True


def start(log: list[str]) -> int:
    log.append("start")
    return 10


def fail_start(log: list[str]) -> int:
    log.append("failed start")
    raise ValueError("start failure")


def add(values: list[int]) -> list[int]:
    values.append(3)
    return [7, 8, 9]


def add_key(values: dict[str, int]) -> list[int]:
    values["b"] = 2
    return [7, 8]


def strict_key(values: dict[str, int]) -> bool:
    values["b"] = 2
    return True


log: list[str] = []
print(sum((item(log, value) for value in source(log) if keep(log, value)), start(log)))
for event in log:
    print(event)
log = []
try:
    print(sum((item(log, value) for value in source(log)), fail_start(log)))
except ValueError as error:
    print(error)
for event in log:
    print(event)

# Ordinary zip sources acquire iterators after all argument expressions run.
mapping = {"a": 1}
for key, number in zip(mapping, add_key(mapping), strict=True):
    print(key)
    print(number)
mapping = {"a": 1}
for key, number in zip(mapping, [7, 8], strict=strict_key(mapping)):
    print(key)
    print(number)

# A generator already acquired its leftmost iterator during argument evaluation.
mapping = {"a": 1}
try:
    for key, number in zip((key for key in mapping), add_key(mapping)):
        print("unexpected mutated generator")
except RuntimeError as error:
    print(error)

# reversed captures its initial cursor before the later zip argument appends.
values = [1, 2]
for left, right in zip(reversed(values), add(values)):
    print(left)
    print(right)
values = [1, 2]
for left, right in zip(values, add(values), strict=True):
    print(left)
    print(right)
