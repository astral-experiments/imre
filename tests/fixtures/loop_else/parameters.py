# ruff: noqa: PLW0120
# Rebinding only in else must still make a parameter an owned local.


def replace_list(values: list[int]) -> None:
    for number in range(0):
        print(number)
    else:
        values = [99]
    values.append(100)
    print(values[0])
    print(values[1])


def replace_string(value: str) -> str:
    while False:
        print("unreachable")
    else:
        value = "replacement"
    return value


values = [1000000000000000000000000000000]
replace_list(values)
print(values[0])
print(len(values))
text = "caller value " + str(27)
print(replace_string(text))
print(text)
