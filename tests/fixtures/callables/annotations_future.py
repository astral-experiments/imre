from __future__ import annotations
from annotationlib import Format, get_annotations


class Point:
    x: int
    text: 'str'

    def __init__(self, x: int, text: str) -> None:
        self.x = x
        self.text = text


class LabelledPoint(Point):
    label: str

    def __init__(self, x: int, text: str, label: str) -> None:
        super().__init__(x, text)
        self.label = label


def describe(a: int) -> 'str':
    return str(a)


def main() -> None:
    print(get_annotations(Point))
    print(get_annotations(LabelledPoint))
    print(get_annotations(describe, format=Format.STRING))
    print(get_annotations(describe))


main()
