import sys
from argparse import ArgumentParser


def read(words: list[str]):
    parser = ArgumentParser()
    parser.add_argument("item")
    return parser.parse_args(words)


def empty():
    parser = ArgumentParser()
    parser.add_argument("--verbose", action="store_true")
    return parser.parse_args(args=[])


def process():
    parser = ArgumentParser()
    parser.add_argument("item")
    return parser.parse_args(None)


print(empty().verbose)
words = ["first"]
print(read(words).item)
print(repr(words))
print(read(sys.argv[1:]).item)
print(process().item)
