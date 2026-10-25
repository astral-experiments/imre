import argparse
import sys


def flat():
    parser = argparse.ArgumentParser(prog="known")
    parser.add_argument("--foo")
    parser.add_argument("-v", action="store_true")
    parser.add_argument("word", nargs="?")
    namespace, extras = parser.parse_known_args()
    print(repr(namespace))
    print(repr(extras))
    extras.append("owned")
    print(repr(extras))


def child(words: list[str]):
    parser = argparse.ArgumentParser(prog="commands")
    parser.add_argument("--parent")
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", aliases=["r"])
    run.add_argument("--flag", action="store_true")
    return parser.parse_known_args(args=words)


def repeat():
    parser = argparse.ArgumentParser(prog="repeat")
    parser.add_argument("--label")
    before, first = parser.parse_known_args(["--before"])
    after, second = parser.parse_known_args(["--label=new", "--after"])
    first.append("independent")
    print(repr(before))
    print(repr(first))
    print(repr(after))
    print(repr(second))
    print(repr(parser.parse_args([])))


if len(sys.argv) > 1 and sys.argv[1] == "commands":
    ns, rest = child(sys.argv[2:])
    print(repr(ns))
    print(repr(rest))
    if ns.command == "r":
        print(ns.flag)
elif len(sys.argv) > 1 and sys.argv[1] == "repeat":
    repeat()
else:
    flat()
