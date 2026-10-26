from functools import cmp_to_key
from io import StringIO


def parity(left: int, right: int) -> int:
    return (left % 2) - (right % 2)


def ignore(left: int, right: int) -> bool:
    return left > right


def fractional(left: float, right: float) -> float:
    return left - right


def text(left: str, right: str) -> int:
    return len(left) - len(right)


def broken(left: int, right: int) -> int:
    print(left)
    print(right)
    raise ValueError("comparison failed")


class Entry:
    rank: int
    stream: StringIO

    def __init__(self, rank: int) -> None:
        self.rank = rank
        self.stream = StringIO(str(rank))


def compare_owned(left: Entry, right: Entry) -> int:
    value = left.rank - right.rank
    left = right
    return value


class Observed:
    def __init__(self, items: list[int]) -> None:
        self.items = items

    def compare(self, left: int, right: int) -> int:
        print(len(self.items))
        self.items.append(99)
        return left - right


def exercise() -> None:
    def descending(left, right):
        return right - left

    print(sorted(range(5), key=cmp_to_key(descending)))
    values = [5, 4, 3, 2, 1, 0]
    print(sorted(values, key=cmp_to_key(parity)))
    print(sorted(values, key=cmp_to_key(parity), reverse=True))
    print(values)
    print(sorted(values, key=cmp_to_key(ignore)))
    truth_key = cmp_to_key(ignore)
    print(truth_key(1) == truth_key(2))
    print(sorted([2.5, -1.0, 0.5], key=cmp_to_key(fractional)))
    print(sorted(["ccc", "dd", "a", "f"], key=cmp_to_key(text)))
    by_none = cmp_to_key(None)
    empty: list[int] = []
    print(sorted(empty, key=by_none))
    print(sorted([12], key=by_none))
    try:
        print(sorted([1, 2], key=by_none))
    except TypeError as error:
        print(str(error))
    try:
        print(sorted([3, 2, 1], key=cmp_to_key(broken)))
    except ValueError as error:
        print(str(error))
    items = [3, 2, 1]
    observer = Observed(items)
    try:
        items.sort(key=cmp_to_key(observer.compare))
    except ValueError as error:
        print(str(error))
    print(items)
    entries = [Entry(3), Entry(1), Entry(2)]
    ordered = sorted(entries, key=cmp_to_key(compare_owned))
    for entry in ordered:
        print(entry.stream.getvalue())
    for entry in entries:
        print(entry.stream.getvalue())
    inputs = [((143 - i) * 17) % 9 for i in range(143)]
    print(sorted(inputs, key=cmp_to_key(parity)))


exercise()
