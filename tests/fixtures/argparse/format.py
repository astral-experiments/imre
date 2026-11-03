import sys
from argparse import ArgumentParser, SUPPRESS
from pathlib import Path
from tempfile import TemporaryDirectory


def render():
    parser = ArgumentParser(prog="printer", description="Hello %(prog)s")
    group = parser.add_argument_group("format")
    group.add_argument("-m", "--mode", metavar="STYLE", choices=["text", "raw"],
                       help="output %(dest)s (%(metavar)s)")
    group.add_argument("--fast", action="store_const", const="yes", metavar="FLAG", help="set %(metavar)s")
    group.add_argument("--choice", choices=["a", "b"], metavar=None, help="label: %(metavar)s")
    print(repr(parser.format_usage()))
    print(repr(parser.format_usage().strip()))
    print(repr(parser.format_help()))
    parser.print_usage(file=sys.stderr)
    print(repr(parser.print_help(None)))
    with TemporaryDirectory() as folder:
        path = Path(folder) / "help.txt"
        with open(path, "w", encoding="utf-8") as stream:
            parser.print_help(stream)
            parser.print_usage(file=stream)
        print(repr(path.read_text()))
        try:
            parser.print_help(stream)
        except ValueError as error:
            print(str(error))
    return parser.parse_args()


def empty_label(words: list[str]):
    parser = ArgumentParser(prog="PROG")
    parser.add_argument("--nil", metavar="", required=True)
    parser.add_argument("--a", metavar="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
    return parser.parse_args(words)


def hidden_labels(words: list[str]):
    parser = ArgumentParser(prog="PROG")
    mutex = parser.add_mutually_exclusive_group()
    mutex.add_argument("--param1", nargs="?", const="default", metavar="NAME", help=SUPPRESS)
    mutex.add_argument("--param2", nargs="?", const="default", metavar="NAME", help=SUPPRESS)
    return parser.parse_args(words)


if len(sys.argv) > 1 and sys.argv[1] == "empty":
    print(repr(empty_label(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "hidden":
    print(repr(hidden_labels(sys.argv[2:])))
else:
    print(repr(render()))
