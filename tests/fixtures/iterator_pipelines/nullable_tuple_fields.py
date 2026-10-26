def values(maybe: int | None) -> tuple[None, int, int | None]:
    return None, 1 << 100, maybe


def consume(parts: tuple[str, str | None]) -> str:
    result = ""
    for part in parts:
        if part:
            result += part
    return result


print(consume(("L", "R")))
print(consume(("L", None)))
for number in values(1 << 140):
    print(number)
for number in reversed(values(None)):
    print(number)
