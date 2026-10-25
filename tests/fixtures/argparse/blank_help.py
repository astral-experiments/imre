import argparse
import sys


def layout() -> str:
    parser = argparse.ArgumentParser(prog="format")
    parser.add_argument("first", choices=[], help="%(choices)s")
    parser.add_argument("following", help="next")
    parser.add_argument("--blank", help="   ")
    parser.add_argument("--empty", help="")
    group = parser.add_argument_group("more")
    group.add_argument("--last", choices=[], help="%(choices)s")
    return parser.format_help()


def whitespace(operation: str) -> None:
    parser = argparse.ArgumentParser(prog="format")
    parser.add_argument("--value", default="  ", help="%(default)s")
    if operation == "format":
        print(parser.format_help())
    elif operation == "print":
        parser.print_help(file=sys.stderr)
    else:
        parser.parse_args(["--help"])


if len(sys.argv) == 1:
    sys.stdout.write(layout())
else:
    try:
        whitespace(sys.argv[1])
    except IndexError as error:
        print(str(error))
