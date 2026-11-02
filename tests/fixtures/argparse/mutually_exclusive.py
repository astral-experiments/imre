import argparse
from argparse import SUPPRESS as hidden_help
import sys


def basic(words: list[str]):
    parser = argparse.ArgumentParser(prog="mutex")
    section = parser.add_argument_group("modes", "Select a mode.")
    mode = section.add_mutually_exclusive_group(required=True)
    mode.add_argument("-a", "--alpha", action="store_true", help="choose alpha")
    parser.add_argument("--outside", help="outside the group")
    mode.add_argument("-b", "--beta", action="store_false", help="choose beta")
    mode.add_argument("-i", "--integer", type=int, help="choose an integer")
    dest = parser.add_mutually_exclusive_group()
    dest.add_argument("--pick", nargs="?", const="P")
    dest.add_argument("file", nargs="?")
    result = parser.parse_known_args(words)
    print(repr(result[0]))
    print(repr(result[1]))


def empty(words: list[str]):
    parser = argparse.ArgumentParser(prog="empty")
    group = parser.add_mutually_exclusive_group(required=True)
    print(repr(parser.parse_args(words)))


def child(words: list[str]):
    parser = argparse.ArgumentParser(prog="commands")
    commands = parser.add_subparsers(dest="command")
    sub = commands.add_parser("run")
    flags = sub.add_mutually_exclusive_group(required=True)
    flags.add_argument("-x", action="store_true")
    flags.add_argument("-y")
    print(repr(parser.parse_args(words)))


def toggle(words: list[str]):
    parser = argparse.ArgumentParser(prog="toggle")
    choice = parser.add_mutually_exclusive_group(required=True)
    choice.add_argument("-f", "--feature", action=argparse.BooleanOptionalAction)
    choice.add_argument("-x", action="store_true")
    print(repr(parser.parse_args(words)))


def suppressed(words: list[str]):
    parser = argparse.ArgumentParser(prog="PROG")
    hidden = parser.add_mutually_exclusive_group()
    hidden.add_argument("--spam", help=argparse.SUPPRESS)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--secret", help=hidden_help)
    mode.add_argument("--visible", help="==SUPPRESS==")
    print(repr(parser.parse_args(words)))


if len(sys.argv) > 1 and sys.argv[1] == "suppressed":
    suppressed(sys.argv[2:])
elif len(sys.argv) > 1 and sys.argv[1] == "empty":
    empty(sys.argv[2:])
elif len(sys.argv) > 1 and sys.argv[1] == "child":
    child(sys.argv[2:])
elif len(sys.argv) > 1 and sys.argv[1] == "toggle":
    toggle(sys.argv[2:])
else:
    basic(sys.argv[1:])
