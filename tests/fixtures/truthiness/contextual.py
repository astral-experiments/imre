class Node:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def conditional(flag: bool) -> list[str]:
    words: list[str] = ([] if flag else ["x"]) or ["y"]
    return words


def newly_bound() -> str:
    result: list[str] = (saved := []) or ["x"]
    saved.append("saved")
    return result[0] + ":" + saved[0]


def selection(optional_node: Node | None) -> list[int] | None:
    return (node := optional_node) and node.values


def composition() -> None:
    sliced: list[str] = [][:] or ["slice"]
    repeated: list[str] = ([] * 2) or ["repeat"]
    print(sliced[0])
    print(repeated[0])


print(conditional(False)[0])
print(conditional(True)[0])
print(newly_bound())
composition()
node = Node([7])
result = selection(node)
if result is not None:
    result.append(8)
print(node.values[1])
print(selection(None) is None)
