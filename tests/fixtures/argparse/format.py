import sys
from argparse import ArgumentParser
from pathlib import Path
from tempfile import TemporaryDirectory


def render():
    parser = ArgumentParser(prog="printer", description="Hello %(prog)s")
    group = parser.add_argument_group("format")
    group.add_argument("-m", "--mode", help="output mode")
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


print(repr(render()))
