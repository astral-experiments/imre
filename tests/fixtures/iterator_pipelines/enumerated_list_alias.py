def grow(values: list[str]) -> int:
    values.append("added")
    return 10


def mutate(values: list[str]) -> None:
    alias = values
    for index, value in enumerate(values):
        if index == 0:
            alias[0] = "replaced"
            grow(alias)
            continue
        if index == 1:
            alias.pop()
            grow(alias)
        print(str(index) + " " + value)
    else:
        print("exhausted")
    print(alias[0])


def start_and_rebind(values: list[str]) -> None:
    alias = values
    for number, value in enumerate(values, grow(alias)):
        print(str(number) + " " + value)
        values = ["unrelated"]
    for number, value in enumerate(alias):
        if value == "added":
            print(str(number))
            break
    else:
        print("no break")


mutate(["first", "second"])
start_and_rebind(["initial"])
