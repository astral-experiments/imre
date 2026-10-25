from itertools import groupby as runs
from pathlib import Path
from typing import TextIO


def sections(values: list[str]) -> list[str]:
    result: list[str] = []
    for title, group in runs(values, key=lambda value: value[0]):
        members = list(group)
        result.append(title + ":" + ",".join(members))
    return result


class Key:
    calls: list[int]

    def __init__(self, calls: list[int]) -> None:
        self.calls = calls

    def observe(self, value: int) -> int:
        self.calls.append(value)
        return value


def mutation_and_lookahead() -> None:
    values = [1, 1, 2, 3]
    calls: list[int] = []
    for key, group in runs(values, Key(calls).observe):
        members = list(group)
        print(f"{key} {len(members)} {len(calls)}")
        if key == 1:
            values[2] = 9
            values[3] = 2
            values.append(4)
            continue
        if key == 2:
            break
    else:
        print("exhausted")
    print(repr(calls))


def fallible(value: int) -> int:
    print(value)
    if value == 9:
        raise StopIteration("key stopped")
    if value < 0:
        raise ValueError("invalid key")
    return value


def failure(values: list[int]) -> None:
    key = 99
    members = [42]
    try:
        for key, group in runs(values, key=fallible):
            members = list(group)
            print(f"body {key} {len(members)}")
        else:
            print("exhausted")
    except ValueError as error:
        print(error)
    print(f"last {key} {members[0]}")


def identity(values: list[int]) -> None:
    for key, group in runs(values):
        members = list(group)
        print(f"{key} {len(members)}")
    else:
        print("finished")


def keep_groups() -> None:
    values = [1, 2]
    kept: list[list[int]] = []
    for key, group in runs(values, key=None):
        members = list(group)
        kept.append(members)
        if key == 2:
            values.append(3)  # The iterator already reached exhaustion.
    print(repr(kept))


class LoggingKey:
    stream: TextIO

    def __init__(self) -> None:
        self.stream = open("keys.txt", "w", encoding="utf-8")

    def observe(self, value: int) -> int:
        self.stream.write("key\n")
        return value


def stop_early() -> None:
    for key, group in runs([1, 2, 3], key=LoggingKey().observe):
        members = list(group)
        break
    # The last group still owns the callback and its buffered file here.
    print(len(Path("keys.txt").read_text()))


print(repr(sections(["alpha", "apple", "beta", "again"])))
mutation_and_lookahead()
failure([-1])
failure([1, 2, -1])
failure([9, 1])
failure([1, 9, 1, 2])
failure([1, 9, 9, 2])
identity([])
identity([1, 1, 2, 1])
keep_groups()
stop_early()
print(len(Path("keys.txt").read_text()))
