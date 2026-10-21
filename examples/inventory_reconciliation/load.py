from .model import Request, Stock


class Reader:
    lines: list[str]
    position: int
    label: str

    def __init__(self, lines: list[str], *, label: str = "stock") -> None:
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

    def count_value(self, token: str) -> int:
        # Short counts stay within the native small-integer range. Delegate long
        # tokens to int() so CPython's configured conversion limit still applies.
        if not token or len(token) > 18:
            return self.number(token)
        sign = token[0]
        digits = token[1:] if sign == "+" or sign == "-" else token
        value = 0
        previous_digit = False
        for character in digits:
            if character == "_":
                if not previous_digit:
                    break
                previous_digit = False
            else:
                value = value * 10 + self.number(character)
                previous_digit = True
        else:
            if previous_digit:
                return -value if sign == "-" else value
        raise ValueError(self.label + ": invalid integer")

    def count(self) -> int:
        fields = self.next()
        if len(fields) != 1:
            raise ValueError(self.label + ": expected count")
        value = self.count_value(fields[0])
        if value < 0:
            raise ValueError(self.label + ": negative count")
        return value

    def item(self) -> tuple[str, int]:
        fields = self.next()
        if len(fields) != 2:
            raise ValueError(self.label + ": expected SKU and quantity")
        return fields[0], self.number(token=fields[1])

    def finish(self) -> None:
        if self.position != len(self.lines):
            raise ValueError(self.label + ": trailing data")


def load(path: str, *, label: str = "stock") -> list[str]:
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
    reader = Reader(lines)
    count = reader.count()
    result: list[Stock] = []
    while count:
        sku, quantity = reader.item()
        if quantity < 0:
            break
        result.append(Stock(sku, quantity=quantity))
        count -= 1
    else:
        reader.finish()
        return result
    raise ValueError("stock: negative quantity")


def read_request(reader: Reader, *, batch: int, row: int) -> Request:
    sku, quantity = reader.item()
    return Request(batch=batch, row=row, sku=sku, quantity=quantity)


def read_orders(lines: list[str]) -> list[list[Request]]:
    reader = Reader(lines, label="orders")
    count = reader.count()
    result: list[list[Request]] = [
        [read_request(reader, batch=batch, row=row) for row in range(reader.count())]
        for batch in range(count)
    ]
    reader.finish()
    return result
