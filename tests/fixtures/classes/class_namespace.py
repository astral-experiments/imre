import copy

events: list[int] = []
values: list[int] = [41]


def make(tag: int) -> list[int]:
    events.append(tag)
    return [tag]


class Defaults:
    values = make(1)
    alias = values
    names = {"red", "blue"}
    numbers = {"first": 1}


class Inherited(Defaults):
    pass


class Replaced(Defaults):
    before = values
    values = make(2)
    after = values


class Unused:
    values = make(3)

class Constructed:
    words = ["class"]
    first: str

    def __init__(self, words: list[str]):
        self.first = self.words[0]
        self.words = words


def choose_receiver(value: Defaults, fail: bool) -> Defaults:
    events.append(4)
    if fail:
        raise ValueError("receiver")
    return value


def choose_value(fail: bool) -> list[int]:
    events.append(5)
    if fail:
        raise ValueError("value")
    return [6]


def base_values(value: Defaults) -> list[int]:
    return value.values


left = Defaults()
right = Inherited()
print(events)
print(left.values is right.values)
right.values.append(7)
print(Defaults.alias)
print(Replaced.alias is Defaults.values)
print(Replaced.values)
right.numbers["second"] = 2
print(left.numbers["second"])
right.names.add("green")
print("green" in Defaults.names)
print(Replaced.before is values)
print(Replaced.after is Replaced.values)
print(base_values(Replaced()))
left.values = [8]
other = copy.copy(left)
left.values.append(9)
print(other.values is left.values)
print(other.values)
print(Defaults.values is right.values)
Defaults.values = [10]
print(right.values)
Inherited.values = [11]
Defaults.values = [12]
print(right.values)
print(left.values)
del left.values
print(left.values is Defaults.values)
try:
    del left.values
except AttributeError as error:
    print(str(error))
events.clear()
try:
    choose_receiver(left, False).values = choose_value(True)
except ValueError as error:
    print(str(error))
print(events)
events.clear()
try:
    choose_receiver(left, True).values = choose_value(False)
except ValueError as error:
    print(str(error))
print(events)
own = Constructed(["instance"])
print(own.first)
print(own.words)
own.words = []
print(own.words)
del own.words
print(own.words is Constructed.words)
