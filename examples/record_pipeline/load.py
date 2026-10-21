from .model import Record


def number(token: str, label: str) -> int:
    digits = token
    if token and (token[0] == "+" or token[0] == "-"):
        digits = token[1:]
    if not digits or len(digits) > 4300:
        raise ValueError(label + ": invalid integer")
    for character in digits.encode():
        if character < 48 or character > 57:
            raise ValueError(label + ": invalid integer")
    return int(token)


def load(path: str) -> list[Record]:
    try:
        with open(path, "r", encoding="utf-8") as source:
            lines = source.read().splitlines()
    except UnicodeDecodeError:
        raise ValueError("records: invalid UTF-8")
    except OSError:
        raise OSError("records: cannot read")
    except ValueError:
        raise ValueError("records: invalid path")
    if not lines:
        raise ValueError("records: missing count")
    fields = lines[0].split()
    if len(fields) != 1:
        raise ValueError("records: expected count")
    count = number(fields[0], "records")
    if count < 0:
        raise ValueError("records: negative count")
    result: list[Record] = []
    position = 1
    while len(result) < count:
        if position >= len(lines):
            raise ValueError("records: truncated")
        fields = lines[position].split()
        if len(fields) != 3:
            raise ValueError("records: expected id, amount and risk")
        identifier = number(fields[0], "records")
        amount = number(fields[1], "records")
        risk = number(fields[2], "records")
        if identifier < 1:
            raise ValueError("records: id must be positive")
        if result and identifier <= result[-1].identifier:
            raise ValueError("records: ids must increase")
        if risk < 0:
            raise ValueError("records: risk must be nonnegative")
        result.append(Record(identifier, amount, risk))
        position += 1
    if position != len(lines):
        raise ValueError("records: trailing data")
    return result
