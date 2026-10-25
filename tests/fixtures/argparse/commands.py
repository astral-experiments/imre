"""Operate on inputs with %(prog)s."""

import argparse
from pathlib import Path


def read() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", default="input", help="Name")
    commands = parser.add_subparsers(dest="command", required=True)
    reader = commands.add_parser("read", help="Read a pair of files")
    reader.add_argument("--first", type=Path)
    reader.add_argument("--second", type=Path)
    write = commands.add_parser("write", help="Write text to a file")
    write.add_argument("--text", required=True, help="Text to write")
    return parser.parse_args()


def show_read(args: argparse.Namespace) -> None:
    if args.first and args.second:
        print(args.first.name + " " + args.second.name)


def show_write(args: argparse.Namespace) -> None:
    print(args.text)


def common(args: argparse.Namespace) -> argparse.Namespace:
    return args


def main() -> None:
    args = read()
    print(repr(args))
    print(args.label)
    alias = args
    match args.command:
        case "read":
            alias = args
            print(common(args).command)
            show_read(args)
        case "write":
            print(common(args).command)
            show_write(args)
    name = alias.command
    match name:
        case "read" | "write":
            print(alias.label)


main()
