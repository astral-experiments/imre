class Item:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


def increment(value: int) -> int:
    return value + 1


def produce(limit: int) -> list[Item]:
    return [Item(increment(value)) for value in range(limit)]


def nested(limit: int) -> dict[int, list[int]]:
    return {
        outer: [increment(inner) for inner in range(outer)] for outer in range(limit)
    }


def unused(values: list[int | None]) -> list[int]:
    return [increment(value) for value in values if value is not None]


# This creates lowering helpers in a checked but inactive module body.
if __name__ == "__main__":
    private = [increment(value) for value in range(3)]
    print(private[0])
