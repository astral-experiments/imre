from dataclasses import dataclass
from typing import BinaryIO


@dataclass
class Labels:
    platform: str
    variant: str


def replace(labels: Labels, generation: int) -> str:
    labels.platform = f"changed-{generation}"
    return labels.variant


def exercise() -> None:
    labels = Labels("", "")
    targets: dict[tuple[str, str], int] = {}
    for generation in range(4):
        labels.platform = f"é-{generation % 2}"
        labels.variant = f"variant-{generation % 2}"
        key = (labels.platform, replace(labels, generation))
        labels.variant = "before lookup"
        found = targets.get(key)
        if found is None:
            labels.platform = "before insertion"
            targets[key] = generation
        else:
            print(found)
    for saved, value in targets.items():
        print(f"{saved[0]} {saved[1]} {value}")


def checked_field(generation: int) -> str:
    if generation == 1:
        raise ValueError("second key component failed")
    return f"value-{generation}"


def failed_replacement() -> None:
    targets: dict[tuple[str, str], int] = {("key-0", "value-0"): 42}
    key = ("unused", "unused")
    for generation in range(2):
        try:
            key = (f"key-{generation}", checked_field(generation))
        except ValueError as error:
            print(str(error))
        print(targets.get(key, -1))


def buffered_marker(mark: bytes) -> BinaryIO:
    stream = open("order.bin", "ab")
    stream.write(mark)
    return stream


def fail() -> int:
    raise ValueError("third argument failed")


def make_key(first: BinaryIO, second: BinaryIO, index: int) -> tuple[str, int]:
    return ("marker", index)


def failed_call() -> None:
    targets: dict[tuple[str, int], int] = {}
    try:
        key = make_key(buffered_marker(b"A"), buffered_marker(b"B"), fail())
        print(targets.get(key, 0))
    except ValueError as error:
        print(str(error))
    with open("order.bin", "rb") as stream:
        print(stream.read())


exercise()
failed_replacement()
failed_call()
