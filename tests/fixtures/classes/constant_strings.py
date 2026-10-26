from collections.abc import Iterable
from dataclasses import dataclass
from _constant_strings import Foreign


class Defaults:
    text = "déjà " "vu"
    cases = ("hi", "ß")
    alias = cases
    empty_suffix = cases + ()
    extended = cases + ("!",)
    repeated = ("hi", "ß")
    empty = ()
    suffix_literal = empty + ("hi", "ß")
    prefix = ("hi",)
    joined = prefix + ("ß",)

    def read(self) -> str:
        return self.text

    @classmethod
    def from_class(cls) -> str:
        return cls.text


class Inherited(Defaults):
    pass


class C:
    label = "first"


class D(C):
    label = "second"


def label(value: C) -> str:
    return value.label


print(C.label, D.label, label(C()), label(D()))


class Other:
    repeated = ("hi", "ß")


def make(log: list[str], fail: bool) -> Defaults:
    log.append("make")
    if fail:
        raise ValueError("no receiver")
    return Inherited()


def read(value: Defaults) -> str:
    return value.text


def cases(value: Defaults) -> tuple[str, str]:
    return value.cases


@dataclass
class Sequences:
    left: Iterable[str]
    right: Iterable[str]


log: list[str] = []
print(Defaults.text)
print(Inherited.text)
print(Inherited().read())
print(Inherited.from_class())
print(read(Inherited()))
print(make(log, False).text)
try:
    print(make(log, True).text)
except ValueError as error:
    print(str(error))
print(log)
print(Defaults.text is Inherited().text)
print(repr(Defaults.extended))
alias = Sequences(cases(Inherited()), Defaults.alias)
suffix = Sequences(Defaults.empty_suffix, Inherited.cases)
extended = Sequences(Defaults.cases, Inherited.extended)
print(alias.left is alias.right)
print(suffix.left is suffix.right)
print(extended.left is extended.right)
for candidate in [Defaults.repeated, Other.repeated, Defaults.suffix_literal, Defaults.joined]:
    pair = Sequences(Defaults.cases, candidate)
    print(pair.left is pair.right)
separate = Sequences(Defaults.cases, Foreign.cases)
empty = Sequences(Defaults.empty, Foreign.empty)
print(separate.left is separate.right)
print(empty.left is empty.right)
