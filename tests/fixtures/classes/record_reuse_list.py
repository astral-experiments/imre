from dataclasses import dataclass
from typing import TextIO


@dataclass
class Holder:
    items: list[int]


@dataclass(repr=False, eq=False)
class Streams:
    items: list[TextIO]


def replacement(items: list[int], index: int) -> list[int]:
    if index == 0:
        return [10]
    if index == 4:
        return [int("bad")]
    return items


def show(items: list[int]) -> None:
    text = ""
    for item in items:
        text += f" {item}"
    print(text)


def lists() -> None:
    previous = Holder([7])
    saved = previous
    contents = previous.items
    try:
        for index in range(5):
            previous = Holder(replacement(previous.items, index))
            previous.items.append(index)
    except ValueError:
        show(previous.items)
        show(saved.items)
        show(contents)


def streams() -> None:
    with open("resource.txt", "w", encoding="utf-8") as handle:
        items = [handle]
        prior = Streams(items)
        for _ in range(3):
            prior = Streams(items)
        print(prior.items[0].closed)
    print(prior.items[0].closed)


lists()
streams()
