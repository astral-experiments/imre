import dataclasses
from typing import Self


@dataclasses.dataclass(frozen=True)
class Wrapped:
    values: dict[str, int]


@dataclasses.dataclass(frozen=True)
class Settings:
    value: Wrapped
    label: str = "unchanged"

    def __post_init__(self) -> None:
        if isinstance(self.value, dict):
            object.__setattr__(self, "value", Wrapped(values=self.value))


@dataclasses.dataclass(frozen=True)
class Options:
    preview: bool = False

    def updated(self: Self, **kwargs) -> Self:
        return type(self)(**{**dataclasses.asdict(self), **kwargs})


@dataclasses.dataclass(frozen=True)
class Check(Options):
    select: str | None = "ALL"


def run() -> None:
    print(Settings({"a": 3}).value.values["a"])
    print(Settings(value=Wrapped({"b": 5})).value.values["b"])
    original = Check(select="E")
    print(original.updated(preview=True))
    print(original.updated(select=None))
    print(original.updated())
    print(original)


run()
