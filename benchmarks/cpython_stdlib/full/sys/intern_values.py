"""Compare exact-string identity, automatic literals, and native argument errors."""
import sys
from enum import StrEnum
from sys import intern as canonical


class State(StrEnum):
    YES = "yes"


def dynamic(number: int) -> str:
    return "never interned before " + str(number)


def run() -> None:
    print(hasattr(sys, "intern"))
    print(hasattr(sys, "_is_interned"))
    print(hasattr(sys, "getdefaultencoding"))
    first = dynamic(721)
    second = dynamic(721)
    print(first == second)
    print(first is second)
    print(sys._is_interned(first))
    print(canonical(first) is first)
    print(sys._is_interned(first))
    print(sys._is_interned(second))
    print(canonical(second) is first)
    print(sys._is_interned(second))
    discard = dynamic(722)
    canonical(discard)
    print(sys._is_interned(discard))
    for value in ("", "!", "é", "Ā", "12_a", "_33", "two words", "abc!", "bleu_é"):
        print(sys._is_interned(value))
    print(sys._is_interned(chr(243)))
    singleton = "é"
    print(canonical(chr(233)) is singleton)
    print(sys._is_interned(State.YES))
    try:
        canonical(State.YES)
    except TypeError as error:
        print(str(error))
    try:
        canonical()
    except TypeError as error:
        print(str(error))
    try:
        sys._is_interned(b"abc")
    except TypeError as error:
        print(str(error))
    try:
        canonical(1, 2)
    except TypeError as error:
        print(str(error))
    try:
        sys._is_interned(extra="abc")
    except TypeError as error:
        print(str(error))


run()
