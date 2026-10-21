"""Source argument order and receiver-before-argument order survive rebinding."""


class Counter:
    number: int
    text: str

    def __init__(self, number: int = 7, *, text: str = "default") -> None:
        self.number = number
        self.text = text

    def combine(self, /, first: int, second: int = 4, *, third: int = 5) -> str:
        self.number += first
        return f"{self.text}:{self.number}:{second}:{third}"


def event(log: list[int], number: int) -> int:
    log.append(number)
    return number


def text_event(log: list[int], number: int) -> str:
    log.append(number)
    return f"text{number}"


def receiver(log: list[int], number: int, counter: Counter) -> Counter:
    log.append(number)
    return counter


def pack(first: int, second: int, third: int) -> str:
    return f"{first}:{second}:{third}"


def absent(log: list[int], number: int) -> None:
    log.append(number)


def receive_none(first: int, second: int | None = None) -> str:
    return f"{first}:{second is None}"


def pair(number: int, text: str) -> tuple[int, str]:
    return (number, text)


def main() -> None:
    log: list[int] = []
    print(pack(event(log, 1), third=event(log, 2), second=event(log, 3)))
    print(pack(third=event(log, 4), first=event(log, 5), second=event(log, 6)))
    counter = Counter()
    print(
        receiver(log, 7, counter).combine(
            third=event(log, 8), first=event(log, 9), second=event(log, 10)
        )
    )
    print(counter.combine(first=1))
    created = Counter(text=text_event(log, 11), number=event(log, 12))
    print(created.combine(second=event(log, 13), first=event(log, 14)))
    print(receive_none(second=absent(log, 15), first=event(log, 16)))
    result = pair(text=text_event(log, 17), number=event(log, 18))
    print(result[0])
    print(result[1])
    for number in log:
        print(number)


main()
