from dataclasses import dataclass, field


@dataclass(slots=True)
class Reading:
    value: int
    hidden: int = field(repr=False, compare=False)


@dataclass(slots=True)
class Other:
    value: int
    hidden: int


@dataclass(slots=True)
class Empty:
    pass


def hold(value: object) -> object:
    return value


def observe(value: object, original: Reading) -> None:
    print(str(value))
    print(repr(value))
    print(bool(value))
    print(isinstance(value, Reading))
    print(isinstance(value, Other))
    print(isinstance(value, tuple))
    print(value is original)
    print(original is not value)
    print(value == original)
    print(original == value)
    print(value == Other(4, 7))
    print(value == (4, 7))
    if isinstance(value, Reading):
        print(value.value)
    try:
        print(len(value))
    except TypeError as exc:
        print(str(exc))
    try:
        print(repr(value[0]))
    except TypeError as exc:
        print(str(exc))


original = Reading(4, 7)
observe(original, original)
observe(Reading(4, 10**4300), original)
observe(Other(4, 7), original)
observe((4, 7), original)
observe(Empty(), original)
alias = hold(original)
original.value = 5
print(repr(alias))
print(hold((True, (False,))) == (1, (0,)))
