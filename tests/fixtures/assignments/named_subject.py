def maybe(choose: bool) -> str | None:
    return "value" if choose else None


def length(choose: bool) -> int:
    match source := maybe(choose):
        case None:
            return -1
        case "value":
            return len(source)
        case _:
            return 0


print(length(False))
print(length(True))

saved: float | None = 1.0
match saved := None:
    case None:
        print(saved is None)
    case 1.0:
        print(saved + 2.0)

match saved := -0.0:
    case None:
        print("wrong absent value")
    case 0.0:
        print(saved)
