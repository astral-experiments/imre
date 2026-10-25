"""Imre's reduce controls pulls and callback invocations over opaque object IDs."""
import sys
from functools import reduce


def request(message: str) -> str:
    print(message)
    sys.stdout.flush()
    value = input()
    if value == "-2":
        raise ValueError("remote exception")
    return value


def pull() -> str:
    return request("next")


def combine(left: str, right: str) -> str:
    return request(f"call {left} {right}")


def main(mode: str) -> None:
    try:
        if mode == "positional":
            result = reduce(combine, iter(lambda: pull(), "-1"), "0")
        elif mode == "keyword":
            result = reduce(combine, iter(lambda: pull(), "-1"), initial="0")
        else:
            result = reduce(combine, iter(lambda: pull(), "-1"))
    except TypeError as error:
        print("type-error " + str(error))
    except ValueError as error:
        print(str(error))
    else:
        print(f"result {result}")


main(sys.argv[1])
