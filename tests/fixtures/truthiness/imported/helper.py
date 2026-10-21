def choose(value: bytes | None) -> bytes:
    return value or b"default"


def count(values: list[int]) -> int:
    if values and len(values):
        return len(values)
    return 0
