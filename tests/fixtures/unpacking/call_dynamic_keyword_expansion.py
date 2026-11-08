def accept(value: int) -> int:
    return value


def forward(key: str) -> int:
    return accept(**{key: 1})


print(forward("value"))
