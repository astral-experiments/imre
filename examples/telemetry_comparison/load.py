from .model import Sample


def number(token: str, label: str) -> int:
    # A bounded ASCII-decimal protocol keeps interchange independent of Unicode
    # database versions. Arithmetic after parsing remains arbitrary precision.
    digits = token
    if token and (token[0] == "+" or token[0] == "-"):
        digits = token[1:]
    if not digits or len(digits) > 4300:
        raise ValueError(label + ": invalid integer")
    for character in digits.encode():
        if character < 48 or character > 57:
            raise ValueError(label + ": invalid integer")
    return int(token)


def load(path: str, label: str) -> list[Sample]:
    try:
        with open(path, "r", encoding="utf-8") as source:
            lines = source.read().splitlines()
    except UnicodeDecodeError:
        raise ValueError(label + ": invalid UTF-8")
    except OSError:
        raise OSError(label + ": cannot read")
    except ValueError:
        raise ValueError(label + ": invalid path")
    if not lines:
        raise ValueError(label + ": missing count")
    fields = lines[0].split()
    if len(fields) != 1:
        raise ValueError(label + ": expected count")
    count = number(fields[0], label)
    if count < 0:
        raise ValueError(label + ": negative count")
    result: list[Sample] = []
    position = 1
    while len(result) < count:
        if position >= len(lines):
            raise ValueError(label + ": truncated")
        fields = lines[position].split()
        if len(fields) != 2:
            raise ValueError(label + ": expected timestamp and value")
        timestamp = number(fields[0], label)
        value = number(fields[1], label)
        if result and timestamp <= result[-1].timestamp:
            raise ValueError(label + ": timestamps must increase")
        result.append(Sample(timestamp, value))
        position += 1
    if position != len(lines):
        raise ValueError(label + ": trailing data")
    return result


def align(reference: list[Sample], observed: list[Sample]) -> None:
    # Exhaust strict zip before any short-circuit query. An alert in the first
    # row must not hide a missing final row in either recording.
    for expected, actual in zip(reference, observed, strict=True):
        if expected.timestamp != actual.timestamp:
            raise ValueError("traces: timestamps differ")
