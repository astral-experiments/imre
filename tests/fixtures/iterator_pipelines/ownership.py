# ruff: noqa: C400
# Materializing a generator must release partial results and captured owners.

from typing import TextIO


class Payload:
    number: int
    text: str
    values: list[int]

    def __init__(self, number: int, text: str, values: list[int]) -> None:
        self.number = number
        self.text = text
        self.values = values


class FileOwner:
    handles: list[TextIO | None]

    def __init__(self, handles: list[TextIO | None]) -> None:
        self.handles = handles


def cache_writer(prefix: str, index: int) -> TextIO:
    stream = open(prefix + str(index) + ".txt", "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write(prefix + str(index))
    return stream


def wrapped_writer(index: int) -> FileOwner:
    return FileOwner([cache_writer("wrapped", index)])


def payload(index: int) -> Payload:
    if index == 2:
        raise ValueError("element failed")
    return Payload(
        1000000000000000000000000000000 + index, "owned " + str(index), [index]
    )


def open_part(prefix: str, index: int) -> TextIO:
    if index == 2:
        raise ValueError("stop opening")
    stream = open(prefix + str(index) + ".txt", "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write(prefix + str(index))
    return stream


def break_files() -> None:
    for stream in (open_part("break", index) for index in range(4)):
        print(bool(stream))
        break


def return_files() -> bool:
    for stream in reversed([open_part("return", 0), open_part("return", 1)]):
        return bool(stream)
    return False


def read_part(path: str) -> int:
    with open(path, "r", encoding="utf-8") as stream:
        text = stream.read()
    print(text)
    return len(text)


def exhausted_frame() -> None:
    # The yielded scalar keeps no file owner. On exhaustion the source list and
    # generator's last private target both disappear before the strict probe.
    try:
        _rows = list(
            zip(
                (0 for stream in [open_part("exhaust", 0)]),
                (read_part("exhaust0.txt") for index in range(2)),
                strict=True,
            )
        )
    except ValueError as error:
        print(error)


def previous_part(prefix: str, index: int) -> int:
    if index > 0:
        read_part(prefix + str(index - 1) + ".txt")
    return index


def cached_results() -> None:
    stream: TextIO | None = None
    for stream, index in zip(
        (open_part("cachedzip", index) for index in range(2)),
        (previous_part("cachedzip", index) for index in range(2)),
    ):
        # zip owns its current tuple after unpacking. Its next pull replaces
        # each old field before advancing the following source.
        stream = None
        assert stream is None
        read_part("cachedzip" + str(index) + ".txt")
    read_part("cachedzip1.txt")
    for index, stream in enumerate(
        open_part("cachedenum", index) for index in range(2)
    ):
        stream = None
        assert stream is None
        read_part("cachedenum" + str(index) + ".txt")
    read_part("cachedenum1.txt")

    # A generator's yielded temporary has no cached tuple retaining it.
    index = 0
    for stream in (open_part("uncached", index) for index in range(2)):
        stream = None
        assert stream is None
        read_part("uncached" + str(index) + ".txt")
        index += 1  # noqa: SIM113 — enumerate would change the ownership under test


def shared_results() -> None:
    held: list[tuple[TextIO, int]] = []
    row: tuple[TextIO, int] | None = None
    for row in zip((cache_writer("escapedzip", index) for index in range(3)), range(3)):
        assert row is not None
        index = row[1]
        if index == 0:
            held.append(row)
        elif index == 1:
            # While shared, the original cache stays rooted and a fresh tuple
            # is yielded. Dropping the external original owner must not flush.
            held[0] = row
            row = None
            read_part("escapedzip0.txt")
        else:
            # Once sole-owned again, the original cached tuple is reused and
            # its first file closes as that field is replaced on the next pull.
            read_part("escapedzip0.txt")
        row = None
        assert row is None
    read_part("escapedzip2.txt")

    owner: FileOwner | None = None
    index = 0
    for (owner,) in zip(wrapped_writer(index) for index in range(2)):
        owner = None
        assert owner is None
        read_part("wrapped" + str(index) + ".txt")
        index += 1  # noqa: SIM113 — the one-field zip cache is under test
    read_part("wrapped1.txt")


def erase_sequence_owner(owners: list[FileOwner | None], index: int) -> int:
    owners[index] = None
    # The earlier zip pull owns the old record before this alias removes it.
    # Its buffered file must remain open until the selected owner is released.
    read_part("sequence0.txt")
    return index


def sequence_resources() -> None:
    owners: list[FileOwner | None] = [FileOwner([cache_writer("sequence", 0)])]
    for owner, index in zip(
        owners,
        (erase_sequence_owner(owners, index) for index in range(1)),
        strict=True,
    ):
        assert owner is not None
        stream = owner.handles[0]
        assert stream is not None
        stream.write(" still open")
        print(owners[index] is None)


def recover() -> None:
    for turn in range(16):
        print(any(payload(index) for index in range(4)))
        print(all(payload(index).number < 0 for index in range(4)))
        try:
            _values = list(payload(index) for index in range(4))
        except ValueError:
            pass
        try:
            _pairs = list(zip((payload(index) for index in range(2)), [1], strict=True))
        except ValueError:
            pass
        try:
            _pairs = list(zip((payload(index) for index in range(4)), [1, 2, 3]))
        except ValueError:
            pass
        values = [payload(0), payload(1)]
        try:
            _owned_pairs = list(
                zip((values.pop() for index in range(2)), reversed(values), strict=True)
            )
        except ValueError:
            pass
        values = [payload(0), payload(1)]
        for value in reversed(values):
            values.pop()
            print(value.number)
            print(value.text)
            print(value.values[0])
        values = [payload(0), payload(1)]
        for left, right in zip((value for value in values), reversed(values)):
            values.pop()
            print(left.text)
            print(right.text)


recover()
print(any(open_part("any", index) for index in range(4)))
try:
    print(all(open_part("all", index) for index in range(4)))
except ValueError as error:
    print(error)
try:
    _opened = list(open_part("list", index) for index in range(4))
except ValueError as error:
    print(error)
print(any(zip((open_part("zip", index) for index in range(4)), [1], strict=True)))
break_files()
print(return_files())
exhausted_frame()
cached_results()
shared_results()
sequence_resources()
read_part("sequence0.txt")
read_part("escapedzip1.txt")
for path in [
    "any0.txt",
    "all0.txt",
    "all1.txt",
    "list0.txt",
    "list1.txt",
    "zip0.txt",
    "break0.txt",
    "return0.txt",
    "return1.txt",
]:
    with open(path, "r", encoding="utf-8") as stream:
        print(stream.read())
