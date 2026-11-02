import itertools as it
from itertools import chain as concatenate


def source(marker: int) -> list[str]:
    print(marker)
    return ["first"]


def fail() -> list[str]:
    print("failed argument")
    raise ValueError("source failure")


def row(value: int) -> list[int]:
    print(f"row {value}")
    return [1 // value]


values = {"old": 1}
for item in concatenate(source(1), values, (), source(2)):
    print(item)
    if item == "first":
        values["new"] = 2
print(sum(concatenate()))
print("|".join(concatenate((), ["a", "b"], (), ["c"])))
try:
    for item in concatenate(source(3), fail()):
        print(item)
except ValueError as error:
    print(str(error))
print(any(concatenate([True], (1 // i > 0 for i in [0]))))

print("".join(concatenate.from_iterable(["abc", "", "def"])))
print(any(it.chain.from_iterable(map(row, [1, 0]))))
rows = [[1, 2], [], [3]]
for value in it.chain.from_iterable(rows):
    if value == 1:
        rows[0].append(4)
        rows.append([5])
    print(value)
