import builtins
from builtins import ascii as escape
from dataclasses import dataclass, field


@dataclass
class Ticket:
    name: str
    calls: list[str] = field(repr=False)

    def __repr__(self) -> str:
        self.calls.append("base")
        return self.name


@dataclass
class ReturnTicket(Ticket):
    def __repr__(self) -> str:
        self.calls.append("return")
        return "↩" + self.name


@dataclass
class Generated(Ticket):
    pass


@dataclass
class Broken(Ticket):
    def __repr__(self) -> str:
        self.calls.append("broken")
        raise ValueError(self.name)


def show(value: Ticket) -> None:
    print(repr(value))
    print(f"{value!r}")
    print(ascii(value))
    print(f"{value!a:}")


def marked(tag: str) -> str:
    print(tag)
    return tag


def main() -> None:
    calls: list[str] = []
    text = "identity" + str(len(calls))
    item = Ticket(text, calls)
    print(ascii(item) is text)
    print(str(item) is text)
    print(builtins.ascii(item) is text)
    print(escape(item) is text)
    show(Generated("été", calls))
    show(item)
    show(ReturnTicket("élève 🐍", calls))
    bad = Broken("échec", calls)
    try:
        print(ascii(bad))
    except ValueError as error:
        print(ascii(error))
    try:
        print(f"{bad!a}")
    except ValueError as error:
        print(f"{error!a}")
    print(calls)
    print(ascii("a'b\"\n\r\t\x00 éΩ🐍"))
    print(ascii((None, False, 2**100, -13.25, b"\x00\xff")))
    print(ascii({"entrée": ["goût", "🐍"]}))
    print(ascii([]))
    print(ascii({}))
    try:
        print(ascii(marked("one"), marked("two")))
    except TypeError as error:
        print(str(error))
    try:
        print(ascii(obj=marked("keyword")))
    except TypeError as error:
        print(str(error))
    try:
        print(ascii())
    except TypeError as error:
        print(str(error))


main()
