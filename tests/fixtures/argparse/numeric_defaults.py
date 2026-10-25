import sys
from argparse import ArgumentParser


def read():
    parser = ArgumentParser(prog="defaults")
    parser.add_argument("word")
    parser.add_argument("--amount", type=int, default="not-an-integer")
    return parser.parse_args()


def ranged(words: list[str]):
    parser = ArgumentParser(prog="range")
    parser.add_argument("--big", type=int, choices=range(0, 5000))
    parser.add_argument("n", type=int, choices=range(100000000000000000003, 99999999999999999998, -2))
    return parser.parse_args(words)


def radix(words: list[str]):
    parser = ArgumentParser(prog="radix")
    parser.add_argument("value", type=int, choices=range(0x1_0000_0000_0000_0000_0000, 0x1_0000_0000_0000_0000_0004, 2))
    parser.add_argument("--flag", type=int, choices=(0x1_0000_0000_0000_0000_0000, -0x1_0000_0000_0000_0000_0000), default=-0x1_0000_0000_0000_0000_0000)
    return parser.parse_args(words)


if len(sys.argv) > 1 and sys.argv[1] == "radix":
    print(repr(radix(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "range":
    print(repr(ranged(sys.argv[2:])))
else:
    print(repr(read()))
