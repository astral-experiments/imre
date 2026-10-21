def optional(mapping: dict[str, int | None]) -> str:
    match mapping:
        case {"value": None}:
            return "present-none"
        case {"value": value} if value is not None:
            return f"present:{value}"
        case {}:
            return "missing"
        case _:
            return "wrong mapping"


def remove(mapping: dict[str, list[int]]) -> bool:
    mapping["alpha"] = [99]
    del mapping["omega"]
    mapping["new"] = [10]
    return True


def aliases(mapping: dict[str, list[int]]) -> None:
    match mapping:
        case {"alpha": selected, **rest} if remove(mapping):
            selected.append(4)
            print(selected[0])
            print(mapping["alpha"][0])
            print(rest["omega"][0])
            rest["beta"].append(7)
            print(len(mapping["beta"]))
            for key, row in rest.items():
                print(f"{key}:{row[0]}:{len(row)}")
        case _:
            print("wrong aliases")


def integers(mapping: dict[int, str]) -> str:
    match mapping:
        case {
            True: first,
            -2: negative,
            1000000000000000000000000000000: large,
            **rest,
        }:
            return first + negative + large + str(len(rest))
        case _:
            return "wrong integers"


def domains(mapping: dict[int, str]) -> str:
    match mapping:
        case {"wrong": value}:
            return value
        case {0: value}:
            return value
        case _:
            return "missing"


print(optional({}))
print(optional({"value": None}))
print(optional({"value": 42}))
aliases({"alpha": [1], "beta": [2], "omega": [3]})
print(integers({1: "a", -2: "b", 1000000000000000000000000000000: "c", 5: "d"}))
print(domains({0: "right"}))
mapping: dict[str, int] = {"one": 1, "two": 2}
match mapping:
    case {**copy}:
        del copy["one"]
        print(len(mapping))
        print(len(copy))
        for key in copy:
            print(key)
match {"é\0🌍": [7]}:
    case {"é\0🌍": [value], **empty}:
        print(value)
        print(len(empty))
maybe: dict[str, int | None] = {"keep": None, "value": 1}
match maybe:
    case {"value": 1, **other}:
        saved = other["keep"]
        print(saved is None)
