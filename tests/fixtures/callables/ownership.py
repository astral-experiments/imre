from typing import Callable, TextIO


class Writer:
    stream: TextIO

    def __init__(self, path: str) -> None:
        self.stream = open(path, "w", encoding="utf-8")

    def write(self, text: str) -> int:
        return self.stream.write(text)


class Payload:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


class Remove:
    values: list[Payload]

    def __init__(self, values: list[Payload]) -> None:
        self.values = values

    def take(self, value: Payload) -> str:
        while len(self.values):
            self.values.pop()
        return value.text


def ignore(text: str) -> int:
    return len(text)


def writer(path: str) -> Callable[[str], int]:
    return Writer(path).write


def read(path: str) -> None:
    with open(path, "r", encoding="utf-8") as stream:
        print(stream.read())


def fail(value: str) -> str:
    if value == "bad":
        raise ValueError("callback failed")
    return value + " kept"


def construct(value: int) -> Payload:
    if value == 2:
        raise ValueError("result failed")
    return Payload("owned result " + str(value))


def failing_argument() -> str:
    raise ValueError("argument failed")


def scan(values: list[Payload], callback: Callable[[Payload], str]) -> int:
    count = 0
    # An indirect callback can invalidate this incoming list's length/buffer.
    for value in (item for item in values):
        print(callback(value))
        count += 1
    return count


def cleanup() -> None:
    fn = writer("bound.txt")
    alias = fn
    fn = ignore
    print(alias("buffered"))
    alias = ignore
    read("bound.txt")
    for turn in range(8):
        try:
            print(writer("argument" + str(turn) + ".txt")(failing_argument()))
        except ValueError:
            print("argument cleanup")
        try:
            partial = list(map(fail, ["owned", "bad", "skipped"]))
            print(len(partial))
        except ValueError:
            print("partial results cleanup")
        try:
            records = list(map(construct, range(4)))
            print(len(records))
        except ValueError:
            print("record results cleanup")
    for value in map(writer("mapped.txt"), ["one", "two"]):
        print(value)
    read("mapped.txt")
    values = [Payload("selected survives"), Payload("discarded")]
    print(scan(values, Remove(values).take))
    print(len(values))


cleanup()
