class Pair:
    first: int
    second: int

    def __init__(self, value: int) -> None:
        self.first = self.second = value


class Row:
    head: int
    rest: list[int]
    left: int
    right: int

    def __init__(self, values: list[int]) -> None:
        self.head, *self.rest = values
        (self.left, (self.right,)) = (10, [11])


class Shared:
    left: list[int]
    right: list[int]

    def __init__(self, values: list[int]) -> None:
        self.left = self.right = values


class Shadow:
    rows: list[list[int]]

    def __init__(self) -> None:
        self.rows = [self for (*self,) in [[20, 21], [22]]]


pair = Pair(1)
print(pair.first)
print(pair.second)
row = Row([2, 3, 4])
print(row.head)
print(len(row.rest))
print(row.rest[0])
print(row.left)
print(row.right)
shared = Shared([5])
shared.left.append(6)
print(shared.right[1])
shared.left = [7]
print(shared.right[0])
shadow = Shadow()
print(shadow.rows[0][1])
print(shadow.rows[1][0])
