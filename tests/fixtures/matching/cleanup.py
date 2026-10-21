def fail() -> bool:
    print("guard")
    raise ValueError("guard failure")


def recover() -> None:
    captured: list[str] = ["old"]
    remaining: dict[str, list[str]] = {}
    try:
        match {"first": ["owned " + str(42)], "last": ["retained " + str(43)]}:
            case {"first": captured, **remaining} if fail():
                print("wrong selected")
            case _:
                print("wrong fallback")
    except ValueError as error:
        print(str(error))
        print(captured[0])
        print(remaining["last"][0])
    finally:
        print("finally")
    captured.append("alive")
    print(len(captured))


def return_through_finally(value: list[list[str]]) -> list[str]:
    try:
        match value:
            case [captured, *rest]:
                print(len(rest))
                return captured
            case _:
                return []
    finally:
        value[0] = ["replacement"]
        print("return finally")


def nested_guard(value: list[int]) -> bool:
    match value:
        case [first, *rest]:
            print(first)
            print(len(rest))
            return True
        case _:
            return False


for iteration in range(20):
    recover()
    rows = [["value " + str(iteration)], ["tail"]]
    returned = return_through_finally(rows)
    print(returned[0])
    print(rows[0][0])
    match [[1000000000000000000000000000000, iteration]]:
        case [row] if nested_guard(row):
            row.pop()
            print(row[0])
        case _:
            print("wrong nested")
