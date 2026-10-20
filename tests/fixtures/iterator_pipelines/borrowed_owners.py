"""Borrowed projections keep their roots, and escaping results acquire owners."""

from typing import TextIO


class Node:
    number: int
    text: str
    weight: float | None

    def __init__(self, number: int, text: str, weight: float | None) -> None:
        self.number = number
        self.text = text
        self.weight = weight

    def reset(self) -> None:
        self.number = 0
        self.text = "replacement"
        self.weight = None


class Envelope:
    node: Node

    def __init__(self, node: Node) -> None:
        self.node = node


class Handle:
    stream: TextIO

    def __init__(self, stream: TextIO) -> None:
        self.stream = stream


def make_node(index: int) -> Node:
    return Node((1 << 100) + index, "borrowed " + str(index), -0.0)


def subtotal(values: list[Node]) -> int:
    return sum(value.number + len(value.text) for value in values)


def first(values: list[Node]) -> Node:
    for value in values:
        alias = value
        return alias
    raise ValueError("empty")


def wrapped(values: list[Node]) -> Envelope:
    for value in values:
        return Envelope(value)
    raise ValueError("empty")


def collected(values: list[Node]) -> list[Node]:
    result: list[Node] = []
    for value in reversed(values):
        result.append(value)
    return result


def saved_fields(values: list[Node]) -> tuple[int, str, float | None]:
    for value in values:
        return value.number, value.text, value.weight
    raise ValueError("empty")


def mixed_owners(values: list[Node]) -> tuple[Node, Node]:
    left = make_node(80)
    right = make_node(81)
    for value in values:
        # Scalarized tuple components can be borrowed, but these destinations
        # also own fresh records and must acquire both transferred references.
        left, right = value, value
        break
    return left, right


def append_while_selected(values: list[Node]) -> str:
    for value in values:
        for index in range(64):
            values.append(make_node(index + 100))
        return value.text
    raise ValueError("empty")


def boxed_float(value: float) -> float | None:
    # This optional conversion allocates; it cannot become an unowned local.
    result: float | None = value
    return result


def drop(values: list[Node]) -> int:
    values.pop()
    return 1


def delegated_drop(values: list[Node]) -> int:
    return drop(values)


def after_pop(values: list[Node]) -> str:
    for value in values:
        delegated_drop(values)
        return value.text
    raise ValueError("empty")


def replace(values: list[Node]) -> int:
    values[0] = make_node(90)
    return 1


def after_replace(values: list[Node]) -> int:
    for value in values:
        replace(values)
        return value.number
    raise ValueError("empty")


def after_reset(values: list[Node]) -> tuple[int, str, float | None]:
    for value in values:
        number, text, weight = value.number, value.text, value.weight
        # Ordinary methods can replace the fields that root earlier projections.
        value.reset()
        return number, text, weight
    raise ValueError("empty")


def rebound_parameter(values: list[Node]) -> str:
    values = [make_node(21)]
    for value in values:
        print(sum(len(values := []) for index in range(1)))
        return value.text
    raise ValueError("empty")


def scoped_pop(values: list[Node]) -> str:
    for value in values:
        text = value.text
        print(sum(delegated_drop(values) for index in range(1)))
        return text
    raise ValueError("empty")


def store_existing(values: list[Node], destination: Envelope) -> str:
    for value in values:
        destination.node = value
        return value.text
    raise ValueError("empty")


def fail_after_projection(values: list[Node]) -> None:
    for value in values:
        alias = value
        try:
            print(alias.number)
            raise ValueError("borrow failed")
        finally:
            print(alias.text)


def writer(path: str) -> TextIO:
    stream = open(path, "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write("opened")
    return stream


def first_stream(values: list[Handle]) -> TextIO:
    for value in values:
        return value.stream
    raise ValueError("empty")


def fail_with_stream(values: list[Handle]) -> None:
    for value in values:
        stream = value.stream
        stream.write(" before error")
        raise ValueError("file borrow failed")


def file_owners() -> None:
    stream = first_stream([Handle(writer("returned.txt"))])
    stream.write(" returned")
    with open("returned.txt", "r", encoding="utf-8") as reader:
        print(len(reader.read()))
    stream.close()
    with open("returned.txt", "r", encoding="utf-8") as reader:
        print(reader.read())
    try:
        fail_with_stream([Handle(writer("failed.txt"))])
    except ValueError as error:
        print(error)
    with open("failed.txt", "r", encoding="utf-8") as reader:
        print(reader.read())


def show_fields(fields: tuple[int, str, float | None]) -> None:
    print(fields[0])
    print(fields[1])
    weight = fields[2]
    if weight is None:
        print("absent")
    else:
        print(weight)


print(subtotal([make_node(0), make_node(1)]))
print(first([make_node(2)]).text)
print(wrapped([make_node(3)]).node.number)
for saved in collected([make_node(4), make_node(5)]):
    print(saved.text)
show_fields(saved_fields([make_node(6)]))
show_fields(saved_fields([Node(0, "no weight", None)]))
left, right = mixed_owners([make_node(20)])
print(left.text)
print(right.text)
print(append_while_selected([make_node(7)]))
optional = boxed_float(-0.0)
if optional is not None:
    print(optional)
print(after_pop([make_node(8)]))
print(after_replace([make_node(9)]))
show_fields(after_reset([make_node(10)]))
print(rebound_parameter([]))
print(scoped_pop([make_node(11)]))
destination = Envelope(make_node(12))
print(store_existing([make_node(13)], destination))
print(destination.node.number)
try:
    fail_after_projection([make_node(14)])
except ValueError as error:
    print(error)
file_owners()
