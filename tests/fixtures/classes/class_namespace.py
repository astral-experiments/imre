import copy
from dataclasses import dataclass
from functools import cached_property

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

@dataclass
class Counted(Defaults):
    count: int

class ImmutableBase:
    marks = ["before"]


@dataclass(frozen=True)
class Immutable(ImmutableBase):
    count: int

class Cached:
    tags = [1]

    @cached_property
    def computed(self) -> list[int]:
        return [7]

class Prepared:
    tags = ["class"]
    count: int

    def __init__(self):
        self.prepare()

    def prepare(self) -> None:
        self.count = 1
        self.tags = ["instance"]


class PreparedChild(Prepared):
    def prepare(self) -> None:
        self.count = 2
        self.tags = ["child"]

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
a = Counted(3)
b = Counted(3)
a.values = [100]
b.values = [200]
print(a == b)
print(repr(a))
del b.values
print(b.values is Defaults.values)
print(a == b)
print(repr(b))
frozen = Immutable(7)
lookup = {frozen: 11}
ImmutableBase.marks = ["after"]
print(frozen.marks)
print(lookup[Immutable(7)])
print(repr(frozen))
cached = Cached()
kept = cached.computed
cached.tags = [3]
print(cached.computed is kept)
print(cached.tags)
del cached.tags
print(cached.tags)
print(cached.computed)
prepared = PreparedChild()
print(prepared.count)
print(prepared.tags)
del prepared.tags
print(prepared.tags is Prepared.tags)
