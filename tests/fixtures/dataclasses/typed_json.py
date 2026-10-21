from dataclasses import dataclass, field
from typing import Self
from relar.json import decode


@dataclass(frozen=True, slots=True)
class Item:
    name: str
    count: int = 2


@dataclass(slots=True, kw_only=True)
class Report:
    total: int
    items: list[Item] = field(default_factory=list)
    optional: str | None = None

    @classmethod
    def parse(cls, text: str) -> Self:
        return decode(text, cls)


@dataclass
class Explicit:
    values: list[int] = field(default_factory=list)

    def __init__(self, values: list[int]) -> None:
        self.values = values


@dataclass
class OptionalCollection:
    values: list[int] | None = field(default_factory=list)


@dataclass(init=False)
class ExplicitDefaults:
    number: int = 7
    values: list[int] = field(default_factory=list)

    def __init__(self, number: int = 11, values: list[int] | None = None) -> None:
        self.number = number
        self.values = [] if values is None else values


first = Report.parse('{"total":1,"items":[{"name":"λ"}]}')
print(first)
second = Report.parse('{"total":2}')
third = Report.parse('{"total":3}')
second.items.append(Item("added"))
print(second)
print(third)
for text in ['{}', '{"total":true}', '{"total":1,"items":null}', '{"total":1,"total":2}']:
    try:
        report = Report.parse(text)
        print("unexpected success")
    except ValueError:
        print("invalid report")
try:
    explicit = decode("{}", Explicit)
    print("unexpected default factory")
except ValueError:
    print("explicit initializer remains required")
print(decode('{"values":[1,2]}', Explicit))
print(decode("{}", OptionalCollection))
print(decode('{"values":null}', OptionalCollection))
print(decode("{}", ExplicitDefaults))
