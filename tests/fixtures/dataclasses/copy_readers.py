from __future__ import annotations

from dataclasses import asdict, dataclass, field


def choose(value: bool) -> bool:
    print(f"choose {value}")
    return value


@dataclass(frozen=True)
class Row:
    name: str = field(kw_only=True)
    flag: bool
    mark: bool | None

    def on(self) -> Row:
        return Row(name=self.name, mark=self.mark, flag=True)

    def off(self) -> Row:
        return Row(**{**asdict(self), "flag": False})

    def blank(self) -> Row:
        return Row(**{**asdict(self), "mark": None})

    def noisy(self) -> Row:
        return Row(**{**asdict(self), "flag": choose(False)})

    def read(self) -> str:
        text = self.name
        if self.flag:
            text += "!"
        if self.mark:
            text += "?"
        return text

    def keep(self) -> list[Row]:
        return [self]


def toggled(row: Row) -> str:
    copy = row.off() if choose(row.flag) else row.on()
    return copy.blank().read()


def direct(row: Row) -> str:
    return row.on().blank().read()


def projected(row: Row) -> bool:
    copy = row.on()
    return copy.flag


def stored(row: Row) -> list[Row]:
    copy = row.off()
    return copy.keep()


def used_twice(row: Row) -> str:
    copy = row.off()
    return copy.read() + copy.read()


def effect(row: Row) -> str:
    copy = row.noisy()
    return copy.read()


def checked(row: Row, fail: bool) -> str:
    def check() -> bool:
        if fail:
            raise ValueError("guard")
        return row.flag

    copy = row.off() if check() else row.on()
    return copy.read()


@dataclass(frozen=True)
class Carrier:
    values: list[int] | None
    flag: bool

    def detach(self) -> Carrier:
        return Carrier(None, self.flag)

    def read(self) -> bool:
        return self.flag


def detached(carrier: Carrier) -> bool:
    copy = carrier.detach()
    return copy.read()



@dataclass(frozen=True)
class Base:
    flag: bool


@dataclass(frozen=True)
class Child(Base):
    owned: list[int]


def changed_base(row: Base) -> Base:
    return Base(False)


def inherited(row: Base) -> bool:
    copy = changed_base(row)
    return copy.flag


@dataclass(frozen=True)
class Hooked:
    flag: bool

    def __post_init__(self) -> None:
        print(f"hook {self.flag}")

    def with_flag(self) -> Hooked:
        return Hooked(True)

    def read(self) -> bool:
        return self.flag


def hook(row: Hooked) -> bool:
    copy = row.with_flag()
    return copy.read()


print(inherited(Child(True, [1])))
print(detached(Carrier([1], True)))
print(hook(Hooked(False)))


for flag in [True, False]:
    original = Row(flag, True, name="a long string that needs to stay alive")
    print(toggled(original))
    print(direct(original))
    print(original.read())
    print(projected(original))
    print(stored(original)[0].read())
    print(used_twice(original))
    print(effect(original))
    print(checked(original, False))
    try:
        checked(original, True)
    except ValueError as exc:
        print(str(exc))
