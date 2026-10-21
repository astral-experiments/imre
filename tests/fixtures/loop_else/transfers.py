# ruff: noqa: PLW0120
# Returns and exceptions must bypass else even without an explicit break.


def first(values: list[str]) -> str:
    for value in values:
        return value
    else:
        return "empty"


def return_while(flag: bool) -> str:
    while flag:
        return "body"
    else:
        return "else"


print(first([]))
print(first(["owned return", "unused"]))
print(return_while(False))
print(return_while(True))
try:
    for number in range(3):
        print(number)
        raise ValueError("for body")
    else:
        print("unreachable")
except ValueError as error:
    print(error)
for number in range(2):
    try:
        raise ValueError("handler break")
    except ValueError as error:
        print(error)
        break
else:
    print("unreachable")
try:
    while True:
        raise ValueError("while body")
    else:
        print("unreachable")
except ValueError as error:
    print(error)
for number in range(3):
    try:
        if number == 1:
            raise ValueError("handled")
        print(number)
    except ValueError:
        continue
else:
    print("handled exception exhausted")
try:
    for number in range(0):
        print(number)
    else:
        raise ValueError("else propagated")
except ValueError as error:
    print(error)
