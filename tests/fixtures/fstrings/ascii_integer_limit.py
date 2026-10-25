from dataclasses import dataclass


@dataclass
class Record:
    values: list[int]


class Custom:
    def __repr__(self) -> str:
        return str(10**4300)


class Fragment:
    def __repr__(self) -> str:
        return f"before {10**4300} {suffix()}"


def suffix() -> str:
    print("suffix")
    return "after"


def render(choice: int, limit: int) -> str:
    if choice == 0:
        return ascii(limit - 1)
    if choice == 1:
        return ascii(1 - limit)
    if choice == 2:
        return ascii(limit)
    if choice == 3:
        return f"{-limit!a}"
    if choice == 4:
        return ascii([1, limit])
    if choice == 5:
        return ascii(({"x": -limit},))
    if choice == 6:
        return ascii(Record([limit]))
    if choice == 7:
        return ascii(ValueError(limit))
    if choice == 8:
        return f"{KeyError(-limit)!a}"
    if choice == 9:
        return ascii(ValueError((limit,)))
    if choice == 10:
        return ascii(ImportError("x", limit))
    if choice == 11:
        return ascii(Custom())
    if choice == 12:
        return f"{Fragment()!a}"
    return ascii("0" * 4301)


def main() -> None:
    boundary = 10**4300
    for i in range(14):
        print(i)
        try:
            print(len(render(i, boundary)))
        except ValueError as error:
            print(str(error))


main()
