class Node:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def bool_fallback(value: bool | None) -> bool:
    return value or True


def bool_selected(value: bool | None) -> bool | None:
    return value and False


def show_bool(value: bool | None) -> None:
    if value is None:
        print("none")
    else:
        print(value)


def float_fallback(value: float | None) -> float:
    return value or 2.5


def float_selected(value: float | None) -> float | None:
    return value and -1.25


def float_rebound(value: float | None) -> float:
    return value or (value := 7.5)


def show_float(value: float | None) -> None:
    if value is None:
        print("none")
    else:
        print(value)


def selected_values(node: Node | None) -> list[int] | None:
    return node and node.values


def absent(node: Node | None) -> None:
    if not node:
        return node
    return None


def bound_in_truth(value: int | None) -> None:
    if bool(value and (saved := 7)):
        print(saved)
    if not bool(value or (other := 0)):
        print(other)


flags: list[bool | None] = [None, False, True]
for flag in flags:
    print(bool_fallback(flag))
    show_bool(bool_selected(flag))
decimals: list[float | None] = [None, -0.0, 0.0, float("nan"), -3.5]
for decimal in decimals:
    print(float_fallback(decimal))
    show_float(float_selected(decimal))
    print(float_rebound(decimal))
node = Node([1])
nodes: list[Node | None] = [None, node]
for optional in nodes:
    result = selected_values(optional)
    if result is None:
        print("absent")
    else:
        result.append(2)
        print(len(node.values))
    print(absent(optional))
bound_in_truth(None)
bound_in_truth(0)
bound_in_truth(4)
