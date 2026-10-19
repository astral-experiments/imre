def reject(values: list[int]) -> bool:
    print("reject")
    values[0] = 10
    values.append(11)
    return False


def guards(values: list[int]) -> None:
    first = -1
    rest: list[int] = []
    match values:
        case [first, *rest] if reject(values):
            print("wrong guard")
        case [10, *later]:
            print(first)
            print(len(rest))
            print(len(later))
        case _:
            print("wrong second case")
    print(first)
    print(rest[0])


def capture_guard(value: str) -> str:
    match value:
        case captured if False:
            return "wrong guard"
        case _:
            return captured


def or_order(pair: tuple[int, int]) -> int:
    match pair:
        case (captured, 0) | (0, captured) if captured > 0:
            return captured
        case _:
            return -1


def branches(values: list[int], choose: bool) -> int:
    match values:
        case [captured] if choose:
            result = captured
        case _:
            result = -1
    return result


def rebound_list(values: list[int]) -> None:
    head = -1
    match values:
        case [head, *values] if False:
            print("wrong guard")
        case [1, 2, 3]:
            print(head)
            print(len(values))
            print(values[0])
        case _:
            print("wrong captured subject")


def rebound_mapping(mapping: dict[str, int]) -> None:
    previous = -1
    match mapping:
        case {"a": previous, **mapping} if False:
            print("wrong guard")
        case {"a": again}:
            print(previous + again)
            print(len(mapping))
            print("a" in mapping)
        case _:
            print("wrong captured mapping")


guards([1, 2])
print(capture_guard("captured before false guard"))
print(or_order((3, 0)))
print(or_order((0, 4)))
print(or_order((0, 0)))
print(branches([8], True))
print(branches([8], False))
rebound_list([1, 2, 3])
rebound_mapping({"a": 1, "b": 2})
value: int | None = 3
while value is not None:
    print(value)
    match None:
        case value:
            pass
if value is None:
    print("none")
else:
    print(value)
for index in range(4):
    try:
        match [index]:
            case [0]:
                continue
            case [2]:
                break
            case [other]:
                print(other)
    finally:
        print(f"finally:{index}")
