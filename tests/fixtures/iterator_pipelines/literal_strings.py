"""Fresh literal iteration preserves control flow and independent string owners."""


def exits(stop: bool) -> str:
    result = ""
    value = "before"
    for value in ["α", "", "α", "\0", "last"]:
        try:
            if value == "":
                continue
            result += value
            if stop and value == "\0":
                break
            value = "rebound"
        finally:
            result += str(len(value))
    else:
        result += "exhausted"
    return result + value


def nested() -> str:
    result = ""
    for outer in ["a", "b"]:
        for inner in ["1", "2", "3"]:
            try:
                if inner == "2":
                    continue
                result += outer + inner
                if outer == "b":
                    break
            finally:
                result += "."
        else:
            result += "inner"
    else:
        result += "outer"
    return result


def returning() -> str:
    for value in ["first", "second"]:
        try:
            return value
        finally:
            print("return cleanup:" + value)
    return "unreachable"


def failing() -> None:
    try:
        for outer in ["a", "b"]:
            for inner in ["1", "2"]:
                try:
                    print(outer + inner)
                    if inner == "2":
                        raise ValueError("failure")
                finally:
                    print("error cleanup:" + inner)
    except ValueError:
        print("caught")


def aliases() -> None:
    values = ["first", "second"]
    for value in values:
        print(value)
        if value == "first":
            values[1] = "changed"
            values.append("added")
    saved: list[str] = []
    for value in (saved := ["a", "b"]):
        print(value)
        if value == "a":
            saved.append("c")
    for value in [*saved]:
        print(value)
    empty: list[str] = []
    for value in empty:
        print("unexpected")
    else:
        print("empty")


def observe(value: str) -> str:
    print("evaluate:" + value)
    return value


def boundaries() -> None:
    values = [outer + inner for outer in ["a", "b"] for inner in ["1", "2"]]
    print("".join(values))
    print("".join(value for value in ["a", "b"]))
    for value in ["one"]:
        print(value)
    for value in ["1", "2", "3", "4", "5", "6", "7", "8"]:
        print(value)
    for value in ["1", "2", "3", "4", "5", "6", "7", "8", "9"]:
        print(value)
    for value in [observe("first"), observe("second")]:
        print("body:" + value)
        break


print(exits(False))
print(exits(True))
print(nested())
print(returning())
failing()
aliases()
boundaries()
