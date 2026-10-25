import sys
from argparse import ArgumentParser


def strict(words: list[str]):
    parser = ArgumentParser(prog="prefix", prefix_chars="+/", allow_abbrev=False)
    parser.add_argument("+x", action="store_true")
    parser.add_argument("+z", action="store_true")
    parser.add_argument("++value", "/v", dest="chosen")
    parser.add_argument("tail", nargs="?")
    return parser.parse_args(words)


def abbreviated(words: list[str]):
    parser = ArgumentParser(prog="multi", add_help=False)
    parser.add_argument("-f")
    parser.add_argument("-foobar", dest="long_name")
    parser.add_argument("-foorab")
    parser.add_argument("-3", dest="three")
    parser.add_argument("-h", action="store_true")
    return parser.parse_args(words)


def bare(words: list[str]):
    parser = ArgumentParser(prog="this-is-a-long-program-with-no-options", add_help=False)
    return parser.parse_args(words)


def positionals(words: list[str]):
    parser = ArgumentParser(prog="p", add_help=False)
    parser.add_argument("one_positional")
    parser.add_argument("another_positional")
    return parser.parse_args(words)


if len(sys.argv) > 1 and sys.argv[1] == "bare":
    print(repr(bare(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "pos":
    print(repr(positionals(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "abbrev":
    print(repr(abbreviated(sys.argv[2:])))
else:
    print(repr(strict(sys.argv[1:])))
