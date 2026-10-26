# Real zero payloads, a retained mutable field, and both CPython IndexErrors.
def select(index: int, first: list[int], second: list[int]) -> list[int]:
    return (first, second)[index]


def check(index: int) -> None:
    first = [1]
    second = [2]
    try:
        found = select(index, first, second)
        print(found is first)
        print(found is second)
        found.append(7)
        print(first)
        print(second)
        print((False, True)[index])
        print((0.0, -0.0)[index])
        print((0, 10**80)[index])
    except IndexError as error:
        print(repr(error))
    try:
        print(()[index])
    except IndexError as error:
        print(str(error))


def alias() -> None:
    a = [1]
    b = [2]
    pair = (a, b)
    result = pair[(pair := (b, a))[0] is b]
    print(result is b)
    print(pair[0] is b)
    try:
        print((1, 2)[2])
    except IndexError as error:
        print(str(error))
    try:
        print((1, 2)[10**40])
    except IndexError as error:
        print(str(error))


for index in [0, 1, -1, -2, -3, 2, 2**63 - 1, -2**63, 2**63, -2**63 - 1, 10**100, -(10**100)]:
    check(index)
alias()
