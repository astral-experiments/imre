"""Absolute-value idioms preserve integer promotion and result ownership."""


def magnitudes(value: int) -> None:
    print(-value if value < 0 else value)
    print(-value if value <= 0 else value)
    print(value if value > 0 else -value)
    print(value if value >= 0 else -value)
    print(-value if 0 > value else value)
    print(-value if 0 >= value else value)
    print(value if 0 < value else -value)
    print(value if 0 <= value else -value)


def transferred(value: int) -> int:
    result = -value if value < 0 else value
    value = 0
    return result


def alternatives(value: int, other: int) -> None:
    print(-value if value < 1 else value)
    print(-value if value == 0 else value)
    print(-value if value < 0 else other)
    print(-other if value < 0 else value)
    print(value if value < 0 else -value)


def observed(value: int) -> int:
    print("observed")
    return value


def effects(value: int) -> int:
    return -observed(value) if value < 0 else value


boundary = 1 << 62
huge = 1 << 150
for value in [
    -huge,
    -boundary - 1,
    -boundary,
    -boundary + 1,
    -1,
    0,
    1,
    boundary - 1,
    boundary,
    huge,
]:
    magnitudes(value)
    print(transferred(value))
    alternatives(value, huge + 7)

print(effects(-huge))
print(effects(huge))
