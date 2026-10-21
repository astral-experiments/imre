def text() -> str:
    return "shared\x00λ"


def concatenate() -> str:
    result = "shared\x00λ"
    for index in range(8):
        result += "shared\x00λ"
    return result


TEXT = "shared\x00λ"
values = [text(), TEXT, "shared\x00λ"]
print(concatenate())
values[0] = values[0] + "changed"
print(values[0])
print(values[1])
print(values[2])
print(text())
try:
    raise ValueError(text())
except ValueError as error:
    print(str(error))
