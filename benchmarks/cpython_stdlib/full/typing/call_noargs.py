from typing import get_overloads, overload


def original() -> None:
    print("called original")


def main() -> None:
    overload(original)
    get_overloads(original)[0]()


main()
