import argparse
import sys


def value(label: str) -> str:
    print(label)
    if label == "interrupt":
        raise RuntimeError(label)
    return label


def read(mode: str):
    parser = argparse.ArgumentParser(prog="register")
    parser.add_argument("--ok", action="store_true")
    try:
        if mode == "const":
            parser.add_argument("--bad", action=argparse.BooleanOptionalAction, const=True)
        elif mode == "invalid":
            parser.add_argument("--no-élan'\n", action=argparse.BooleanOptionalAction)
        elif mode == "both":
            parser.add_argument("--no-feature", action=argparse.BooleanOptionalAction, nargs=2, const=True)
        elif mode == "effects":
            parser.add_argument("--bad", action="count", help=value("first"), choices=value("second"))
        elif mode == "interrupt":
            parser.add_argument("--bad", action="store_false", const=value("interrupt"), help=value("late"))
    except TypeError as error:
        print("type: " + str(error))
    except ValueError as error:
        print("value: " + str(error))
    except RuntimeError as error:
        print("runtime: " + str(error))
    return parser.parse_known_args(["--bad", "--ok"])


def nested(mode: str):
    parser = argparse.ArgumentParser(prog="nested", prefix_chars="+")
    group = parser.add_argument_group("parent options")
    group.add_argument("++ok", action="store_true")
    commands = parser.add_subparsers(dest="command")
    child = commands.add_parser("run")
    child_group = child.add_argument_group("child options")
    child_group.add_argument("--on", action="store_true")
    try:
        if mode == "nested-group":
            group.add_argument("++bad", action="count", choices=value("group"))
        elif mode == "nested-child":
            child.add_argument("--no-bad", action=argparse.BooleanOptionalAction)
        else:
            child_group.add_argument("--bad", action="store_true", const=value("interrupt"), help=value("late"))
    except TypeError as error:
        print(str(error))
    except ValueError as error:
        print(str(error))
    except RuntimeError as error:
        print(str(error))
    return parser.parse_known_args(["++ok", "run", "--on", "--bad"])


def mutex_registration(mode: str):
    parser = argparse.ArgumentParser(prog="mutex", prefix_chars="+")
    titled = parser.add_argument_group("options")
    flags = titled.add_mutually_exclusive_group()
    flags.add_argument("++ok", action="store_true")
    commands = parser.add_subparsers(dest="command")
    child = commands.add_parser("run")
    child_flags = child.add_mutually_exclusive_group(required=True)
    child_flags.add_argument("--on", action="store_true")
    try:
        if mode == "mutex-group":
            flags.add_argument("++bad", action="count", choices=value("group"))
        elif mode == "mutex-child-name":
            child_flags.add_argument("--no-off", action=argparse.BooleanOptionalAction)
        else:
            child_flags.add_argument("--bad", action="store_true", const=value("interrupt"), help=value("late"))
    except TypeError as error:
        print(str(error))
    except ValueError as error:
        print(str(error))
    except RuntimeError as error:
        print(str(error))
    return parser.parse_known_args(["++ok", "run", "--on", "--bad"])


mode = sys.argv[1] if len(sys.argv) > 1 else "absent"
if mode.startswith("mutex-"):
    print(repr(mutex_registration(mode)))
elif mode.startswith("nested-"):
    print(repr(nested(mode)))
else:
    print(repr(read(mode)))
