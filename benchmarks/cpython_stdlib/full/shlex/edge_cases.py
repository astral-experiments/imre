"""Exercise quote boundaries, both lexer modes, and iterator evaluation order."""
import shlex
from collections import deque
from enum import StrEnum


class Word(StrEnum):
    EMPTY = ""
    ARG = "two words"


def observe(word: str) -> None:
    print(repr(shlex.quote(word)))
    for posix in [False, True]:
        for comments in [False, True]:
            try:
                print(repr(shlex.split(word, comments=comments, posix=posix)))
            except ValueError as exc:
                print("!" + str(exc))


def mark(value: int, seen: list[int]) -> int:
    seen.append(value)
    return value


def main() -> None:
    # All words of length 0..4 in this alphabet, including malformed quotes and escapes.
    words = [""]
    for length in range(5):
        new: list[str] = []
        for word in words:
            observe(word)
            if length != 4:
                for char in ["a", " ", "'", '"', "\\", "#", "\n", "é"]:
                    new.append(word + char)
        words = new
    for code in range(128):
        observe(chr(code))
    for word in ["零", "🙂", "a\u0301", "line\r#x\nword", "a\v\f\tb"]:
        observe(word)
    print(shlex.join(split_command=("two words", "'", "")))
    print(shlex.join(x for x in ["", "two words", "'"]))
    empty: list[str] = []
    queue: deque[int] = deque()
    assert shlex.split(Word.ARG) == ["two", "words"]
    assert shlex.join([Word.ARG, Word.EMPTY]) == "'two words' ''"
    assert shlex.quote(queue) == "''"
    assert shlex.join([queue, queue]) == "'' ''"
    print(shlex.join(empty))
    print(shlex.join([]))
    print(shlex.quote(empty))
    print(shlex.quote(b""))
    print(shlex.quote(False))
    print(shlex.quote(-0.0))
    for integer in [0, 10**55]:
        try:
            print(shlex.quote(integer))
        except TypeError as exc:
            print(str(exc))
    seen: list[int] = []
    try:
        print(shlex.join(mark(x, seen) for x in [0, 1, 2]))
    except TypeError as exc:
        print(str(exc))
    print(repr(seen))


main()
