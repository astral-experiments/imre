import sys
from argparse import ArgumentParser


def optional(words: list[str]):
    parser = ArgumentParser(prog="pack")
    parser.add_argument("-v", action="store_true")
    commands = parser.add_subparsers(dest="command")
    run = commands.add_parser("run", aliases=["r", "start"], help="launch a job")
    run.add_argument("--value")
    show = commands.add_parser(name="list", aliases=("ls",), help="list the jobs")
    show.add_argument("name", nargs="?")
    return parser.parse_args(words)


def absent(words: list[str]):
    parser = ArgumentParser(prog="pack")
    commands = parser.add_subparsers()
    child = commands.add_parser("run")
    child.add_argument("--mark", action="store_true")
    return parser.parse_args(words)


def required(words: list[str]):
    parser = ArgumentParser(prog="pack")
    commands = parser.add_subparsers(required=True)
    run = commands.add_parser("run", aliases=["r"])
    run.add_argument("--value")
    commands.add_parser("list")
    return parser.parse_args(words)


def named(words: list[str]):
    parser = ArgumentParser(prog="pack")
    commands = parser.add_subparsers(
        dest="verb", metavar="COMMAND", help="verb for %(prog)s",
        title="jobs", description="Actions provided by %(prog)s.",
    )
    commands.add_parser(name="go", aliases=("g", "go", "g", "go-now"), help="start a job")
    commands.add_parser("two words", aliases=["", "Δ"])
    commands.required = True
    return parser.parse_args(words)


def empty(words: list[str]):
    parser = ArgumentParser(prog="pack")
    parser.add_subparsers()
    return parser.parse_args(words)


def empty_required(words: list[str]):
    parser = ArgumentParser(prog="pack")
    parser.add_argument("--key", required=True)
    parser.add_subparsers(required=True)
    return parser.parse_args(words)


def dispatch(words: list[str]) -> None:
    namespace = optional(words)
    if namespace.command == "r":
        print(repr(namespace.value))
    elif "ls" == namespace.command:
        print(repr(namespace.name))
    else:
        print(repr(namespace.command))


if len(sys.argv) > 1 and sys.argv[1] == "absent":
    print(repr(absent(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "required":
    print(repr(required(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "named":
    print(repr(named(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "empty":
    print(repr(empty(sys.argv[2:])))
elif len(sys.argv) > 1 and sys.argv[1] == "dispatch":
    dispatch(sys.argv[2:])
elif len(sys.argv) > 1 and sys.argv[1] == "empty-required":
    try:
        print(repr(empty_required(sys.argv[2:])))
    except TypeError as error:
        print(str(error))
else:
    print(repr(optional(sys.argv[1:])))
