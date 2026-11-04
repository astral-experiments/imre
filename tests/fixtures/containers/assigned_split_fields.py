def read(events: list[int]) -> str:
    events.append(1)
    return "|é||末|"


def captured(events: list[int]) -> list[str]:
    fields = read(events).split("|")
    events.append(2)
    result: list[str] = []
    for field in fields:
        result.append(field)
    return result


def until_stop(text: str) -> list[str]:
    fields = text.split("|")
    text = "replaced"
    result: list[str] = []
    for field in fields:
        if field == "stop":
            break
        if field == "skip":
            continue
        result.append(field)
    else:
        result.append("exhausted")
    return result


def aliased(text: str) -> list[str]:
    fields = text.split("|")
    alias = fields
    alias[0] = "changed"
    result: list[str] = []
    for field in fields:
        result.append(field)
    return result


def twice(text: str) -> list[str]:
    fields = text.split("|")
    result: list[str] = []
    for field in fields:
        result.append(field)
    for field in fields:
        result.append(field)
    return result


def limited(text: str, separator: str) -> list[str]:
    fields = text.split(separator, 1)
    result: list[str] = []
    for field in fields:
        result.append(field)
    return result


events: list[int] = []
print("~".join(captured(events)))
print(events[0])
print(events[1])
print("~".join(until_stop("a|skip|é|stop|z")))
print("~".join(until_stop("")))
print("~".join(aliased("first|last")))
print("~".join(twice("a||b")))
print("~".join(limited("a||b", "|")))
