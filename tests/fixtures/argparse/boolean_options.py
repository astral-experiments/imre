from argparse import ArgumentParser, BooleanOptionalAction
import sys


def read():
    parser = ArgumentParser(prog="switches")
    parser.add_argument("-x", "--feature", "--extra", action=BooleanOptionalAction, help="enable feature")
    parser.add_argument("--quiet", action=BooleanOptionalAction, default=True)
    parser.add_argument("-v", action="count")
    return parser.parse_args()


def prefixed(words: list[str]):
    parser = ArgumentParser(prog="prefixed", prefix_chars="-+/")
    parser.add_argument("+β", "++mode", "--enable", dest="value", action=BooleanOptionalAction)
    parser.add_argument("/verbose", action=BooleanOptionalAction, default=False)
    return parser.parse_args(words)


if len(sys.argv) > 1 and sys.argv[1] == "prefix":
    print(repr(prefixed(sys.argv[2:])))
else:
    print(repr(read()))
