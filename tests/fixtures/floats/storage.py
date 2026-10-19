class Reading:
    value: float
    optional: float | None
    values: list[float]

    def __init__(self, value: float = -0.0, optional: float | None = None) -> None:
        self.value = value
        self.optional = optional
        self.values = [value]

    def add(self, delta: int) -> float:
        self.value += delta
        return self.value


def report(value: float | None) -> str:
    return "none" if value is None else f"{value!s}:{value:}"


def defaults(value: float = -0.0, *, other: float | None = 1e400) -> str:
    return f"{value}:{report(other)}"


def pair(value: float) -> tuple[float, float | None]:
    return value, value


def from_empty() -> list[float]:
    return [][:]


def cleanup(value: float | None, replace: bool) -> float | None:
    try:
        return value
    finally:
        if replace:
            return -0.0


def fail() -> float:
    raise ValueError("late field")


def mixed_success(value: Reading, amount: int) -> float:
    value.value += amount
    return value.value


class PartialReading:
    optional: float | None
    last: float

    def __init__(self, value: float) -> None:
        self.optional = value
        self.last = fail()


values: list[float | None] = [None, 0.0, -0.0, 1.5, float("nan"), float("inf")]
copied = values[::-1] * 2
values[0] = -2.5
for value in copied:
    print(report(value))
print(report(values.pop()))
print(report(values[2]))
print(defaults())
print(defaults(other=None, value=3.25))

reading = Reading(optional=-0.0)
alias = reading
print(reading.value)
print(report(reading.optional))
print(reading.add(2))
alias.optional = float("nan")
print(report(reading.optional))
reading.values.append(2.75)
reading.values[0] += 1
reading.values[1] /= 2
for value in reading.values:
    print(value)
reading.value *= 0.5
reading.value //= 0.1
print(reading.value)

records = [Reading(-0.0), Reading(2.0, float("inf"))]
for record in records:
    print(record.value)
    print(report(record.optional))

mapping: dict[str, float] = {"zero": -0.0, "finite": 1.75, "nan": float("nan")}
mapping["finite"] += 2
mapping["finite"] /= 2
for key, value in mapping.items():
    print(f"{key}:{value}")
print(report(mapping.get("zero")))
print(report(mapping.get("missing")))
print(mapping.get("missing", 2.5))
print(report(mapping.get("nan", None)))
optional_mapping: dict[str, float | None] = {"none": None, "zero": -0.0}
print(report(optional_mapping.get("none")))
print(report(optional_mapping.get("zero")))
print(report(optional_mapping.get("missing", 1.25)))

left, right = pair(-0.0)
print(left)
print(report(right))
tuples: list[tuple[float, float | None]] = [pair(1.25), pair(float("nan"))]
for first, second in tuples:
    print(first)
    print(report(second))
empty = from_empty()
empty.append(0.5)
print(empty[0])
generated = [float(index) / 2 for index in range(5)]
for value in generated[::2]:
    print(value)
generated_mapping = {str(index): float(index) + 0.25 for index in range(3)}
for key, value in generated_mapping.items():
    print(f"{key}:{value}")

for value in copied:
    print(report(cleanup(value, False)))
    print(report(cleanup(value, True)))
try:
    partial: list[float | None] = [-0.0, 1.5, fail()]
except ValueError:
    print("partial list cleaned")
try:
    partial_tuple = (-0.0, copied[1], fail())
except ValueError:
    print("partial tuple cleaned")
try:
    Reading(fail(), copied[2])
except ValueError:
    print("failed constructor arguments")
try:
    PartialReading(-0.0)
except ValueError:
    print("partial optional field cleaned")

huge = 1
for power in range(400):
    huge *= 10
try:
    print(mixed_success(reading, huge))
except OverflowError:
    print("field conversion propagated")
print(reading.value)
