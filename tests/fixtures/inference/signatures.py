def entry(value):
    return middle(value) + 2


def middle(value):
    return tail(value) * 3


def tail(value):
    return value - 1


def unused_literal():
    return "not executed"


def unused_typed(values: list[str]):
    return values[:]


def unused_default(value=4):
    return value + 1


def record(value):
    print(value)


def nothing():
    pass


def choose(flag: bool):
    if flag:
        return "yes"
    return "no"


def preserve_optional(value: int | None):
    return value


def pair(value):
    return value, "row"


def numbers(value):
    return [value, value + 1]


def exact_result(value) -> int:
    return value


print(entry(6))
print(entry(-3))
print(exact_result(7))
record("recorded once")
print(nothing())
print(choose(False))
print(choose(True))
optional = preserve_optional(None)
if optional is None:
    print("empty")
else:
    print(optional)
optional = preserve_optional(9)
if optional is None:
    print("empty")
else:
    print(optional)
number, text = pair(11)
print(number)
print(text)
for value in numbers(20):
    print(value)
