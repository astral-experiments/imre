from collections.abc import Callable
from dataclasses import dataclass, field


@dataclass
class Label:
    name: str
    events: list[str] = field(repr=False)

    def __str__(self) -> str:
        self.events.append(f"str {self.name}")
        if self.name == "bad":
            raise ValueError("bad label")
        return self.name


def make(name: str, events: list[str]) -> Label:
    events.append(f"make {name}")
    return Label(name, events)


def main() -> None:
    events: list[str] = []
    print(str(make("one", events)))
    print(f"{make('two', events)} {make('three', events)!s} {make('four', events)!r}")
    item = make("five", events)
    print(repr(item))
    print(repr([item]))
    print(item)
    print(item.__str__())
    print(Label.__str__(item))
    callback: Callable[[], str] = make("six", events).__str__
    print(callback())
    retained = str(make(f"owned №{len(events)}", events))
    print(f"{make('next', events)} {retained}")
    try:
        print(
            f"{make('seven', events)} {make('bad', events)!s} {make('never', events)}"
        )
    except ValueError as error:
        print(str(error))
    print(repr(events))


main()
