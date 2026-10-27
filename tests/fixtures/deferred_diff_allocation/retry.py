"""Recover every owner after refusing the allocation used by one list observation."""
from difflib import ndiff
from itertools import tee
from sys import argv

from imre import struct


class RowsError(Exception):
    def __init__(self, value, *args):
        self.value = value
        super().__init__(value, *args)


def truth(item: object) -> bool:
    return bool(item)


def union_truth(value: list[str] | str) -> bool:
    return bool(value)


@struct
class PackedRow:
    value: float
    enabled: bool


def packed_rows(mode: str, shape: str) -> None:
    rows = [PackedRow(1.5, True), PackedRow(2.5, False)]
    result: list[PackedRow] = []
    count = 0 if shape == "empty" else 2
    print("armed")
    for attempt in range(2):
        try:
            if mode == "packed-slice":
                result = rows[:count]
            else:
                result = rows * count
            print("ok")
            break
        except MemoryError as error:
            print("refused")
            print(repr(error))
    for row in rows:
        print(row.value)
        print(row.enabled)
    print(len(result))
    for row in result:
        print(row.value)
        print(row.enabled)


def check(mode: str, shape: str) -> None:
    if mode == "packed-slice" or mode == "packed-repeat":
        packed_rows(mode, shape)
        return
    left = [argv[3], argv[4]]
    right = [argv[4], argv[3]]
    if shape == "sparse":
        left = [str(i + 10000) for i in range(1024)]
        right = list(left)
        right[-1] = "Z"
    rows = list(ndiff(left, right))
    alias = list(rows)
    first, second = tee(rows)
    boxed = RowsError(rows).args[0]
    left[0] = "changed after snapshot"
    right[0] = "also changed after snapshot"
    if mode == "restore":
        changes = [v[2:] for v in rows if v.startswith("+ ")]
        if len(changes) < 0:
            raise RuntimeError("invalid projection")
    if mode == "tee-boundary" or mode == "tee-cached":
        for i in range(57):
            next(first)
        if mode == "tee-cached":
            for i in range(57):
                next(second)
    number = 0
    value = ""
    print("armed")
    for attempt in range(2):
        try:
            if mode == "len" or mode == "restore":
                number = len(rows)
            elif mode == "copy-during":
                alias = list(rows)
                number = len(alias)
            elif mode == "copy":
                number = len(alias)
            elif mode == "slice-owner":
                alias = rows[1:]
            elif mode == "index":
                value = rows[0]
            elif mode == "tee-first" or mode == "tee-boundary":
                value = next(first)
            elif mode == "tee-second" or mode == "tee-cached":
                value = next(second)
            elif mode == "reverse":
                rows.reverse()
            elif mode == "repr":
                value = repr(rows)
            elif mode == "union":
                if union_truth(rows):
                    number = 1
            elif mode == "frozen":
                number = len(frozenset(rows))
            elif mode == "boxed":
                if truth(boxed):
                    number = 1
            elif mode == "pack-project":
                selected = [v[2:] for v in rows if v.startswith("+ ")]
                number = len(selected)
            elif mode == "join-owner":
                value = ",".join(rows)
            elif mode == "project":
                changes = [v for v in rows if v.startswith(("+", "-"))]
                number = len(changes)
            print("ok")
            break
        except MemoryError as error:
            print("refused")
            print(repr(error))
    print(number)
    print(repr(value))
    print(repr(rows))
    print(repr(alias))
    print(repr(list(first)))
    print(repr(list(second)))

check(argv[1], argv[2])
