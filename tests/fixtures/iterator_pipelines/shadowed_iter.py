from collections.abc import Callable


def iter(producer: Callable[[], str], stop: str) -> list[str]:
    return [stop, producer()]


for value in iter(lambda: "produced", "stop"):
    print(value)
