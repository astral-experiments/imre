"""Earlier field values remain alive across later mutations and failure cleanup."""


class Box:
    text: str
    number: int

    def __init__(self, text: str, number: int) -> None:
        self.text = text
        self.number = number


class Owner:
    box: Box

    def __init__(self, box: Box) -> None:
        self.box = box


def replace(owner: Owner) -> str:
    owner.box = Box("new owner", 0)
    return "replaced"


def remove_text(values: list[str]) -> str:
    values.pop()
    return "removed text"


def remove_number(values: list[int]) -> None:
    values.pop()


def fail(log: list[int]) -> str:
    log.append(1)
    raise ValueError("late field failed")


def unreachable(log: list[int]) -> str:
    log.append(99)
    return "must not run"


def main() -> None:
    owner = Owner(Box("sole owned " + "string", 1000000000000000000000000000007))
    print(f"{owner.box.text}:{owner.box.number}:{replace(owner)}:{owner.box.text}")
    texts = ["sole owned " + "list element"]
    print(f"{texts[0]}:{remove_text(texts)}")
    numbers = [1000000000000000000000000000000000000000000000000000000000017]
    print(f"{numbers[0]}:{remove_number(numbers)}:{len(numbers)}")
    numbers = [1000000000000000000000000000000000000000000000000000000000019]
    print(f"{numbers.pop()}:{len(numbers)}")
    print(f"{'left ' + 'temporary'}:{1000000000000000000000000000000 + 23}")
    failures: list[int] = []
    for turn in range(24):
        texts = ["owned string " + "before failure"]
        numbers = [1000000000000000000000000000000000000000000000000000000000029]
        try:
            print(
                f"{texts[0]}:{remove_text(texts)}:{numbers.pop()}:{fail(failures)}:{unreachable(failures)}"
            )
        except ValueError as problem:
            if turn == 0:
                print(f"caught:{problem}")
        try:
            print(f"{turn}:{f'inner:{fail(failures)}'}:{unreachable(failures)}")
        except ValueError:
            pass
    print(len(failures))
    print(failures[-1])


main()
