"""Failing later keywords clean up evaluated arguments, receivers, and file owners."""

from typing import TextIO


class Box:
    text: str
    number: int

    def __init__(self, text: str, number: int) -> None:
        self.text = text
        self.number = number

    def receive(self, *, number: int) -> str:
        return f"{self.text}:{number}"


def fail(log: list[int]) -> int:
    log.append(1)
    raise ValueError("later keyword failed")


def unexpected(log: list[int]) -> int:
    log.append(99)
    return 0


def consume(last: int, words: list[str], number: int) -> int:
    return last + len(words) + number


def opened() -> TextIO:
    stream = open("partial.txt", "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write("argument was evaluated")
    return stream


def close_file(*, stream: TextIO, number: int) -> int:
    stream.close()
    return number


def main() -> None:
    log: list[int] = []
    for turn in range(24):
        try:
            consume(
                words=["owned " + "argument"],
                number=1000000000000000000000000000000000000000000000031,
                last=fail(log),
            )
        except ValueError:
            pass
        try:
            Box(text="temporary " + "receiver", number=turn).receive(number=fail(log))
        except ValueError:
            pass
        try:
            Box(text="temporary " + "constructor argument", number=fail(log))
        except ValueError:
            pass
    try:
        close_file(stream=opened(), number=fail(log))
    except ValueError as problem:
        print(problem)
    with open("partial.txt", "r", encoding="utf-8") as stream:
        print(stream.read())
    try:
        consume(last=fail(log), number=unexpected(log), words=[])
    except ValueError:
        pass
    print(len(log))
    print(log[-1])


main()
