from itertools import chain as concatenate


def source(marker: int) -> list[str]:
    print(marker)
    return ["first"]


def fail() -> list[str]:
    print("failed argument")
    raise ValueError("source failure")


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
