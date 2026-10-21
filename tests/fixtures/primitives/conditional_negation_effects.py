"""Conditional negation keeps float signs and observable evaluation effects."""


def floating(value: float) -> None:
    print(-value if value < 0 else value)
    print(value if 0 <= value else -value)


def observed(value: int) -> int:
    print("condition")
    return value


def condition_call(value: int) -> int:
    return -value if observed(value) < 0 else value


def branch_binding(value: int) -> None:
    result = -value if value < 0 else (value := value + 1)
    print(result)
    print(value)


def condition_binding(value: int) -> None:
    result = -value if (value := value + 1) < 0 else value
    print(result)
    print(value)


for value in [float("-inf"), -1.5, -0.0, 0.0, 1.5, float("inf"), float("nan")]:
    floating(value)
for number in [-(1 << 150), -1, 0, 1, 1 << 150]:
    print(condition_call(number))
    branch_binding(number)
    condition_binding(number)
