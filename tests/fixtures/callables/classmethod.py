class Base:
    @classmethod
    def pick(cls, n: int = 21) -> int:
        return n + 1

    @classmethod
    def choose(cls, n: int) -> int:
        return cls.number() + n

    @staticmethod
    def number() -> int:
        return 11


class Child(Base):
    @staticmethod
    def number() -> int:
        return 29


def original() -> Child:
    print("receiver")
    return Child()


def argument() -> int:
    print("argument")
    return 3


def from_closure() -> int:
    saved = Child.choose

    def inner() -> int:
        if saved is Child.choose:
            return Base.number()
        return saved(3)

    return inner()


first = Child.pick
second = Child.pick
alias = first
raw = first.__func__
print(first is second, first is alias, first.__self__ is Child)
print(raw is second.__func__, raw is Base.pick.__func__)
print(first(), second(21), raw(Child), raw(Base, 21))
print(original().choose(argument()), from_closure())
