def length(source: str | None) -> int:
    match source:
        case "first" if len([source for source in range(0)]) > 0:
            return 0
        case "second":
            return len(source)
        case _:
            return -1


print(length("first"))
print(length("second"))
print(length(None))
