class Reading:
    value: float | None

    def __init__(self, value: float | None) -> None:
        self.value = value


def identity(value: float) -> float:
    return value


def return_capture(value: float | None) -> float:
    match value:
        case 0.0 as captured:
            return captured
        case _:
            return -1.0


def use_capture(reading: Reading) -> float:
    match reading:
        case Reading(value=0.5 as captured):
            reading.value = None
            assigned: float = captured
            items: list[float] = [captured]
            total = 0.0
            total += captured
            assert captured > 0.0
            print(-captured)
            return identity(captured) + assigned + items[0] - captured + total
        case _:
            return -1.0


print(return_capture(-0.0))
print(return_capture(0.0))
print(return_capture(None))
reading = Reading(0.5)
print(use_capture(reading))
print(reading.value is None)
print(use_capture(Reading(None)))
print(use_capture(Reading(1.5)))
