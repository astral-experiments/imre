events: list[int] = []


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
    values = make(2)


class Unused:
    values = make(3)


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
