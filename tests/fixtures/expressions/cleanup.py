"""Selected arms, slices, and string iterators release owners on every transfer."""

from typing import TextIO


def fail() -> list[str]:
    raise ValueError("selected arm failure")


def fail_bound() -> int:
    raise ValueError("slice bound failure")


def discard_return(flag: bool) -> list[str]:
    try:
        return ["owned " + str(42)] if flag else ["other " + str(43)]
    finally:
        raise ValueError("discarded conditional return")


def replace_return() -> list[str]:
    try:
        return ["first " + str(42), "second " + str(43)][::-1]
    finally:
        return ["replacement"] if True else []  # noqa: B012, SIM107 — exercise displaced return cleanup


def write_stream(path: str) -> TextIO:
    stream = open(path, "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write(path)
    return stream


def discard_stream() -> TextIO:
    try:
        return write_stream("selected.txt") if True else write_stream("unused.txt")
    finally:
        raise ValueError("discarded file return")


def fail_iteration(text: str) -> None:
    for character in text:
        if character == "🌍":
            raise ValueError("iteration failure")


def main() -> None:
    for turn in range(24):
        try:
            _selected = ["left " + str(turn)] if False else fail()
        except ValueError:
            pass
        try:
            _slice = ["owned " + str(turn)][
                1000000000000000000000000000000 : fail_bound()
            ]
        except ValueError:
            pass
        try:
            _text = ("owned " + str(turn))[None : fail_bound()]
        except ValueError:
            pass
        try:
            _character = ("owned " + str(turn))[1000000000000000000000000000000]
        except IndexError:
            pass
        try:
            discard_return(turn % 2 == 0)
        except ValueError:
            pass
        try:
            fail_iteration("owned " + str(turn) + "é🌍tail")
        except ValueError:
            pass
        try:
            _partial = [10 // (1 if character == "a" else 0) for character in "aé"]
        except ZeroDivisionError:
            pass
        replacement = replace_return()
        print(replacement[0])
    try:
        discard_stream()
    except ValueError:
        print("discarded file owner")
    with open("selected.txt", "r", encoding="utf-8") as stream:
        print(stream.read())
    first = write_stream("first.txt")
    second = write_stream("second.txt")
    streams = [first, second]
    reversed_streams = streams[::-1]
    reversed_streams[0].close()
    print(second.closed)
    first.close()
    print("recovered")


main()
