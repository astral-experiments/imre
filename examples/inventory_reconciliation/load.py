from .model import Request, Stock


class Reader:
    lines: list[str]
    position: int
    label: str

    def __init__(self, lines: list[str], label: str) -> None:
        self.lines = lines
        self.position = 0
        self.label = label

    def next(self) -> list[str]:
        if self.position >= len(self.lines):
            raise ValueError(self.label + ": truncated")
        fields = self.lines[self.position].split()
        self.position += 1
        return fields

    def number(self, token: str) -> int:
        try:
            return int(token)
        except ValueError:
            raise ValueError(self.label + ": invalid integer")

    def count(self) -> int:
        fields = self.next()
        if len(fields) != 1:
            raise ValueError(self.label + ": expected count")
        value = self.number(fields[0])
        if value < 0:
            raise ValueError(self.label + ": negative count")
        return value

    def item(self) -> tuple[str, int]:
        fields = self.next()
        if len(fields) != 2:
            raise ValueError(self.label + ": expected SKU and quantity")
        return fields[0], self.number(fields[1])

    def finish(self) -> None:
        if self.position != len(self.lines):
            raise ValueError(self.label + ": trailing data")


def load(path: str, label: str) -> list[str]:
    try:
        with open(path, "r", encoding="utf-8") as source:
            return source.read().splitlines()
    except UnicodeDecodeError:
        raise ValueError(label + ": invalid UTF-8")
    except OSError:
        raise OSError(label + ": cannot read")
    except ValueError:
        raise ValueError(label + ": invalid path")


def read_stock(lines: list[str]) -> list[Stock]:
    reader = Reader(lines, "stock")
    count = reader.count()
    result: list[Stock] = []
    for _ in range(count):
        sku, quantity = reader.item()
        if quantity < 0:
            raise ValueError("stock: negative quantity")
        result.append(Stock(sku, quantity))
    reader.finish()
    return result


def read_request(reader: Reader, batch: int, row: int) -> Request:
    sku, quantity = reader.item()
    return Request(batch, row, sku, quantity)


def read_orders(lines: list[str]) -> list[list[Request]]:
    reader = Reader(lines, "orders")
    count = reader.count()
    result: list[list[Request]] = [
        [read_request(reader, batch, row) for row in range(reader.count())]
        for batch in range(count)
    ]
    reader.finish()
    return result
