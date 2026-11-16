class Verdict:
    first: bool
    calls: int

    def __init__(self, first: bool):
        self.first = first
        self.calls = 0

    def __bool__(self) -> bool:
        self.calls += 1
        return self.first if self.calls == 1 else not self.first


def otherwise(flag: bool, value: Verdict, other: Verdict) -> Verdict:
    return (other if flag else (value and other)) and other


def when_true(flag: bool, value: Verdict, other: Verdict) -> Verdict:
    return ((value and other) if flag else other) and other


first = Verdict(False)
second = Verdict(False)
other = Verdict(True)
print(otherwise(False, first, other) is first, first.calls)
print(when_true(True, second, other) is other, second.calls)
