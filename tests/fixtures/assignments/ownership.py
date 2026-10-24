class Item:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


def show(first: Item, second: Item, third: Item) -> None:
    print(first.text + ":" + second.text + ":" + third.text)


def fail() -> str:
    raise ValueError("late failure")


def argument(first: str, second: str) -> None:
    print(first + second)


def execute() -> None:
    item = Item("initial")
    show(item, (item := Item("next")), item)
    for index in range(100):
        text = "left " + str(index)
        print(text == (text := "right " + str(index)))
        value = int("1000000000000000000000000000000000000000") + index
        print(value < (value := value + 1) < value + 1)
    text = "before"
    try:
        argument(text, (text := fail()))
    except ValueError as error:
        print(str(error))
    print(text)
    try:
        argument(text, (text := "published") + fail())
    except ValueError as error:
        print(str(error))
    finally:
        print(text)
    optional: Item | None = None
    print((optional := Item("optional")).text)
    if optional is not None:
        print(optional.text)
    print((optional := None))
    values = [1, 2, 3]
    alias = values
    outputs = [(values := [index]) for index in range(3)]
    print(alias[2])
    print(values[0])
    print(outputs[0][0])
    print(outputs[2][0])


def rebound_integer(number: int) -> int:
    result = number + (number := number + 1)
    print(number)
    return result


def paired_parameter(text: str) -> tuple[str, str]:
    return ((text := text + " changed"), text)


def first_name(rows: list[str]) -> str:
    rows: str = rows[0]
    return rows


execute()
print(first_name(["row " + str(4)]))
first, second = paired_parameter("tuple")
print(first)
print(second)
small = 1
print((small := int("100000000000000000000000000000000000000000000000000000")))
print(small + 1)
print(rebound_integer(small))
value = 10
value += (value := 20)
print(value)
values = [10]
old = values
values[0] += (values := [30])[0]
print(values[0])
print(old[0])
