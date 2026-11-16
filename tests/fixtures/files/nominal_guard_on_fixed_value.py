class Record:
    def amount(self) -> int:
        return 0


def increment(value: int) -> int:
    if isinstance(value, Record):
        return value.amount()
    return value + 1


print(increment(41))
