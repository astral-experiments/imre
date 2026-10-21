"""Fields run left to right, including None expressions and comprehension helpers."""


def event(log: list[int], number: int) -> int:
    log.append(number)
    return number


def nothing(log: list[int], number: int) -> None:
    log.append(number)


def main() -> None:
    log: list[int] = []
    print(f"{event(log, 1)}:{nothing(log, 2)}:{event(log, 3)!s}")
    print(f"{event(log, 4)}:{f'{event(log, 5)}:{event(log, 6)}'}:{event(log, 7)}")
    print("left:" + f"{event(log, 8)}:{event(log, 9)}" + ":right")
    print(f"{event(log, 10)} adjacent {event(log, 11)}")

    target = "outer"
    words = [f"{target}:{event(log, target + 20)}" for target in range(3)]
    for word in words:
        print(word)
    print(target)
    values: list[int | None] = [None, 4, None, 5]
    narrowed = [f"{value}" for value in values if value is not None]
    for word in narrowed:
        print(word)
    print(f"comprehension:{[f'{value}' for value in range(2)][1]}")
    nested = [[f"{outer}:{inner}" for inner in range(outer)] for outer in range(3)]
    for row in nested:
        for word in row:
            print(word)
    for number in log:
        print(number)


main()
