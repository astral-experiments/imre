class Rank:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        return self.value < other.value

    def __eq__(self, other):
        raise AssertionError("unexpected equality comparison")


class BrokenReverse:
    def __bool__(self):
        raise ValueError("reverse")


class BrokenIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError("iterator")


def broken_key(value):
    raise ValueError("key")


def current():
    unsafe = bytes((*range(0x21), 0x25, 0x7F)).decode()
    for extra in ("", "&=+#", "/?#", ":@/?#"):
        print([ord(c) for c in sorted(unsafe + extra)])

    rows = [(i % 5, i) for i in range(96)]
    seen = []

    def key(row):
        seen.append(row[1])
        return Rank(row[0])

    result = sorted((row for row in rows), key=key, reverse=1)
    print([row[1] for row in result], seen == list(range(96)))

    values = [3, 1, 2]

    def mutate(value):
        values.pop()
        return -value

    print(sorted(values, key=mutate), values)
    print(sorted([], key=1), sorted("βaA", key=None, reverse=None))

    for call in (
        lambda: sorted(),
        lambda: sorted(iterable=[]),
        lambda: sorted([1], None),
        lambda: sorted([], unknown=1),
        lambda: sorted(1),
        lambda: sorted([0], key=1),
        lambda: sorted([0], key=broken_key),
        lambda: sorted([1, "s"]),
        lambda: sorted([], reverse=BrokenReverse()),
        lambda: sorted(BrokenIterator(), unknown=1),
    ):
        try:
            call()
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
