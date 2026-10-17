class Counter:
    total: int

    def __init__(self, total: int) -> None:
        self.total = total

    def bump(self, amount: int) -> int:
        self.total += amount
        return self.total


def create(total: int) -> Counter:
    return Counter(total)
