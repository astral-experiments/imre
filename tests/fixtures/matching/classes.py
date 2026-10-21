class Cell:
    label: str
    values: list[int]
    enabled: bool | None

    def __init__(self, label: str, values: list[int], enabled: bool | None) -> None:
        self.label = label
        self.values = values
        self.enabled = enabled

    def count(self) -> int:
        match self:
            case Cell(values=[first, *rest]):
                return first + len(rest)
            case _:
                return 0


class Box:
    cell: Cell

    def __init__(self, cell: Cell) -> None:
        self.cell = cell


class Empty:
    def __init__(self) -> None:
        pass


class OtherCell:
    label: str

    def __init__(self, label: str) -> None:
        self.label = label


def classify(cell: Cell | None) -> str:
    match cell:
        case OtherCell(label=other):
            return other + ":wrong nominal identity"
        case None:
            return "none"
        case Cell(enabled=True, values=[first, *rest], label=label) as whole:
            whole.values.append(9)
            return f"{label}:{first}:{len(rest)}:{len(cell.values)}"
        case Cell(enabled=None, label=label):
            return label + ":unknown"
        case Cell():
            return "disabled or empty"
        case _:
            return "wrong class"


def replace(cell: Cell) -> bool:
    cell.values = [99]
    cell.label = "replacement"
    return True


def nominal_or(cell: Cell | None) -> str:
    match cell:
        case (OtherCell() | Cell()) as whole:
            return whole.label
        case None:
            return "none"
        case _:
            return "wrong nominal OR"


def stable(cell: Cell) -> None:
    match cell:
        case Cell(values=captured, label=label) if replace(cell):
            print(captured[0])
            print(label)
            print(cell.values[0])
        case _:
            print("wrong guard")


print(classify(None))
print(classify(Cell("ready", [1, 2, 3], True)))
print(classify(Cell("waiting", [], None)))
print(classify(Cell("off", [], False)))
cell = Cell("owned", [1000000000000000000000000000000], True)
box = Box(cell)
match box:
    case Box(cell=Cell(values=row) as captured):
        box.cell = Cell("new", [9], False)
        row.append(2)
        print(captured.label)
        print(captured.values[0])
        print(len(cell.values))
        print(box.cell.label)
match Empty():
    case Empty():
        print("empty class")
print(cell.count())
stable(Cell("owned " + str(42), [1000000000000000000000000000000], True))
print(nominal_or(cell))
print(nominal_or(None))
