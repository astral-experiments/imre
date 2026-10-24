from typing import BinaryIO


class Measurement:
    kind: bytes
    value: float | None

    def __init__(self, kind: bytes, value: float | None) -> None:
        self.kind = kind
        self.value = value


def number(value: float | None) -> str:
    return "none" if value is None else str(value)


def scalar_rebinding() -> None:
    value = 1.25
    print(value + (value := 2.5))
    print(value)
    data = b"before"
    print(data + (data := b"after"))
    print(data)
    optional: float | None = None
    print((optional := -0.0) / -1.0)
    saved: list[float | None] = [optional, (optional := 2.0), optional]
    print(optional := None)
    for held in saved:
        print(number(held))


def kibibytes(bytes: int) -> float:
    bytes: float = float(bytes)
    bytes /= 1024.0
    return abs(bytes)


def guard_rebinding() -> None:
    value = 1.5
    captured = 0.0
    match value:
        case 1.5 as captured if (value := 2.5) < 0.0:
            print("wrong float guard")
        case 1.5:
            print(captured + value)
    data = b"original"
    previous = b""
    match data:
        case b"original" as previous if (data := b"changed") == b"never":
            print("wrong bytes guard")
        case b"original":
            print(previous + data)
    optional: float | None = 1.0
    match optional:
        case 1.0 if (optional := None) is not None:
            print("wrong optional guard")
        case 1.0:
            print(number(optional))


def numeric_patterns(value: int) -> str:
    match value:
        case 1.0:
            return "one"
        case 9007199254740992.0:
            return "exact large integer"
        case _:
            return "different integer"


def pattern_keys() -> None:
    match True:
        case 1.0:
            print("boolean numeric pattern")
    match 1.0:
        case True:
            print("wrong singleton")
        case 1:
            print("float numeric pattern")
    values: dict[int, bytes] = {0: b"zero", 1: b"one", 9007199254740993: b"large"}
    match values:
        case {0.5: _}:
            print("wrong fractional key")
        case {1e400: _}:
            print("wrong infinite key")
        case {9007199254740992.0: _}:
            print("wrong rounded key")
        case {1.0: first, -0.0: second}:
            print(first + second)
    tags: dict[bytes, float] = {b"T": -0.0}
    match tags:
        case {b"T": reading}:
            print(reading)


def byte_numbers() -> None:
    for data in [b"1_2.5", b"-0", b"\xff", b"\xc2\xa01", b"\xd9\xa1"]:
        try:
            print(float(data))
        except UnicodeDecodeError:
            print("wrong decoding exception")
        except ValueError:
            print("invalid byte number")


def parse_record(line: str) -> Measurement:
    match line.split():
        case [tag, "missing"]:
            return Measurement(tag.encode(), None)
        case [tag, text]:
            return Measurement(tag.encode(), float(text))
        case _:
            raise ValueError("malformed measurement")


def summarize(source: BinaryIO) -> bytes:
    count = 0
    total = 0.0
    skipped = 0
    latest: float | None = None
    for line in (payload := source.read()).decode().splitlines():
        match parse_record(line):
            case Measurement(kind=b"T", value=None):
                skipped += 1
            case Measurement(kind=b"T", value=0.0 as zero):
                count += 1
                total += (latest := zero)
                print(f"zero:{zero}")
            case Measurement(kind=b"T", value=sample) if sample is not None:
                count += 1
                total += (latest := sample)
            case Measurement(kind=b"H"):
                skipped += 1
            case _:
                raise ValueError("unknown measurement")
    average: float | None = None
    if count > 0:
        average = total / count
    return f"count={count} mean={number(average)} last={number(latest)} skipped={skipped} bytes={len(payload)}".encode()


scalar_rebinding()
print(kibibytes(bytes=-2048))
guard_rebinding()
for integer in [1, 9007199254740992, 9007199254740993]:
    print(numeric_patterns(integer))
pattern_keys()
byte_numbers()
payload = b"T 12.5\nT -0.0\nT missing\nH 50.0\nT 2.5\n"
with open("measurements.bin", "wb") as writer:
    print(writer.write(payload))
with open("measurements.bin", "rb") as reader:
    summary = summarize(reader)
    print(reader.read() == b"")
with open("summary.bin", "wb") as output:
    output.write(summary)
print(summary.decode())
