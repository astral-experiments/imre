import secrets
from secrets import choice


class Sequence:
    def __init__(self, item):
        self.item = item
        self.events = []

    def __len__(self):
        self.events.append("len")
        return 3 if len(self.events) == 1 else 1

    def __getitem__(self, index):
        self.events.append(index)
        return self.item

    def __bool__(self):
        raise AssertionError("choice called bool")


class LenOnly:
    def __len__(self):
        return 1

    def __iter__(self):
        raise AssertionError("choice called iter")


def current():
    item = []
    seq = Sequence(item)
    assert choice is secrets.choice
    assert choice(seq=seq) is item and seq.events == ["len", "len", 0]
    assert choice([item]) is item and choice((item,)) is item
    assert choice({0: item}) is item
    assert choice("🎉") == "🎉" and choice(b"\xff") == 255
    assert choice(bytearray(b"x")) == 120
    assert 0 <= choice(range(1 << 40)) < 1 << 40
    for call in (
        lambda: choice([]),
        lambda: choice(()),
        lambda: choice(4),
        lambda: choice({1}),
        lambda: choice({1: item}),
        lambda: choice(LenOnly()),
        lambda: choice(),
        lambda: choice(1, 2),
        lambda: choice([1], seq=[1]),
        lambda: choice(value=[1]),
    ):
        try:
            call()
        except (IndexError, TypeError, KeyError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("expected choice to raise")

    class Broken(Sequence):
        def __getitem__(self, index):
            raise IndexError("custom index")

    class BrokenLength(Sequence):
        def __len__(self):
            if self.events:
                raise ValueError("second length")
            self.events.append("len")
            return 1

    for seq in (Broken(item), BrokenLength(item)):
        try:
            choice(seq)
        except (IndexError, ValueError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("expected sequence error")


def entropy():
    try:
        print(choice("abc"))
    except OSError as error:
        print(type(error).__name__, error.args, error.errno, error.strerror, str(error))
