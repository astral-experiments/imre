def mark(value: int) -> int:
    print(value)
    return value


def unused() -> None:
    range()


try:
    range(mark(1), mark(2), mark(3), mark(4))
except TypeError as error:
    print(str(error))

try:
    print(list(range(stop=mark(5))))
except TypeError as error:
    print(str(error))

try:
    for number in range():
        print(number)
except TypeError as error:
    print(str(error))
