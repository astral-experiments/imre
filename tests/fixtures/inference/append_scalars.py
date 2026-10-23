def render(values: list[int]) -> str:
    rows = []
    alias = rows
    for value in values:
        rows.append(f"{value}!")
    alias.append("end")
    return ",".join(rows)


def flags(enabled: bool) -> list[bool]:
    items = []
    if enabled:
        items.append(enabled)
    return items


def packets(data: bytes) -> bytes:
    items = []
    items.append(data)
    return items[0]


def measures(number: float) -> float:
    items = []
    items.append(number)
    return items[0]


def fallback() -> int:
    items = []
    items.extend([1])
    return items[0]


def scoped() -> int:
    items = []

    def inner(text: str) -> str:
        items = []
        items.append(text)
        return items[0]

    items.append(3)
    print(inner("inner"))
    return items[0]


print(render([]))
print(render([0, -2, 5]))
print(len(flags(False)))
print(flags(True)[0])
print(packets(b"data"))
print(measures(2.5))
print(fallback())
print(scoped())
