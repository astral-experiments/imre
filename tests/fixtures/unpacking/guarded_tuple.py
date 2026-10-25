from typing import cast


def record(parts: list[str]) -> tuple[str, str, str]:
    if len(parts) != 3:
        raise ValueError("bad width")
    parts[0] = parts[0].replace(":", "/")
    value = cast("tuple[str, str, str]", tuple(parts))
    return value


def only_empty(parts: list[int]) -> tuple[()]:
    if 0 != len(parts):
        raise ValueError("expected empty")
    return tuple(parts)


parts = ["a:b", "hash", "size"]
result = record(parts)
parts[0] = "changed"
print(result[0])
print(result[1])
try:
    record(["bad"])
except ValueError as exc:
    print(str(exc))
print(len(only_empty([])))
