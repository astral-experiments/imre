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


print(repr(read(sys.argv[1] if len(sys.argv) > 1 else "absent")))
