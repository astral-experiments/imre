class Box:
    values: list[int]

    def __init__(self, values: list[int]) -> None:
        self.values = values


def show(first: list[int], second: list[int]) -> None:
    print(first[0])
    print(second[0])


def strings(first: str, second: str) -> str:
    return first + ":" + second


def replace(box: Box) -> list[int]:
    box.values = [99]
    return [88]


def fail() -> list[int]:
    raise ValueError("selected failure")


for index in range(20):
    values = [1000000000000000000000000000000 + index]
    show(values or [1], (values := [2]))
    values = []
    show(values or (values := [3]), values)
    box = Box([4])
    old = box.values
    show(box.values or replace(box), replace(box))
    print(old[0])
    print(box.values[0])
    text = "owned " + str(index)
    print(strings(text or "fallback", (text := "replacement")))
    print((Box([5]) or Box([6])).values[0])
    print((Box([7]).values and Box([8]).values)[0])
    print(("" or ("temporary " + str(index))) + "!")
    print((b"" or bytes([index])) + b"!")
    optional: list[int] | None = [9]
    result = optional or (optional := [10])
    result.append(11)
    if optional is not None:
        print(len(optional))
    try:
        show(result or [12], fail())
    except ValueError as error:
        print(error)
    print(result[0])
    try:
        empty: list[int] = []
        selected = empty or fail()
    except ValueError:
        print("cleaned empty")
