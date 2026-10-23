def classify(delta: float) -> str:
    return f"{delta < 0} {0 <= delta} {delta == 0} {delta != 0} {delta >= 0} {0 < delta}"


def positive(*, delta: float, label: str = "ok") -> str:
    if delta > 0:
        raise ValueError(label)
    return label


def read(log: list[str], value: int) -> int:
    log.append("argument")
    return value


def read_label(log: list[str]) -> str:
    log.append("label")
    return "positive"


def inferred(value):
    return classify(value)


def main() -> None:
    huge = 1
    for step in range(400):
        huge *= 10
    for count in [0, 1, -1, 9007199254740993, -huge, huge]:
        print(inferred(count))
    for number in [-0.0, 0.0, -5e-324, 5e-324, float("inf"), float("-inf"), float("nan")]:
        print(classify(number))
    log: list[str] = []
    try:
        print(positive(delta=read(log, huge), label=read_label(log)))
    except ValueError as error:
        print(str(error))
    try:
        print(positive(label=read_label(log), delta=read(log, huge) // 0))
    except ZeroDivisionError:
        print("division")
    print(repr(log))


main()
