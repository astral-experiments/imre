from argparse import ArgumentParser
from enum import Enum
from pathlib import Path


class Mode(Enum):
    text = "text"
    alias = "text"
    json = "json"


def parse():
    p = ArgumentParser(description="Collect positional paths.")
    p.add_argument("-v", "--verbose", action="store_true")
    p.add_argument(
        "-c", "--mode", choices=[item.value for item in Mode], default=Mode.alias,
        help="Select %(choices)s (default: %(default)s)",
    )
    p.add_argument("command", choices=["check", "format"], help="Action to run")
    p.add_argument("baseline", type=Path)
    p.add_argument("comparison", type=Path)
    return p.parse_args()


args = parse()
print(args.command)
print(str(args.baseline))
print(str(args.comparison))
print(args.verbose)
print(isinstance(args.mode, str))
print(Mode(args.mode).value)
