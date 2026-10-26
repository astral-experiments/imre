"""Emit each actual call, pull, caught exception and exhaustion in order."""
import sys
from _string import formatter_field_name_split, formatter_parser


def probe(text: str) -> None:
    print("parse")
    p = formatter_parser(text)
    print(iter(p) is p)
    for _ in range(len(text) + 3):
        try:
            print(repr(next(p)))
        except ValueError as error:
            print("error:" + str(error))
        except StopIteration:
            print("end")
            print(next(p, None) is None)
            break
    print("split")
    try:
        first, rest = formatter_field_name_split(text)
    except ValueError as error:
        print("initial error:" + str(error))
        return
    if isinstance(first, int):
        print(repr(first))
    else:
        print(repr(first))
    print(iter(rest) is rest)
    for _ in range(len(text) + 3):
        try:
            print(repr(next(rest)))
        except ValueError as error:
            print("error:" + str(error))
        except StopIteration:
            print("end")
            print(next(rest, None) is None)
            break


if sys.argv[1] == "--one":
    probe(sys.argv[2])
elif sys.argv[1] == "--integer-one":
    try:
        formatter_parser(1)
    except TypeError as error:
        print(str(error))
    try:
        formatter_field_name_split(1)
    except TypeError as error:
        print(str(error))
else:
    with open(sys.argv[1], newline="") as inputs:
        for line in inputs:
            probe(line[:-1])
    # Newline is the external corpus delimiter, so exercise it in-process.
    probe("pass\n{0!\n:{x}}.end")
    probe("x\r{0}")
