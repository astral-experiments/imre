from argparse import ArgumentParser


def read():
    parser = ArgumentParser(prog="counts")
    parser.add_argument("-v", "--flag", action="store_true")
    parser.add_argument("--pair", nargs=2)
    parser.add_argument("--maybe", nargs="?", const="present", default="missing")
    parser.add_argument("--items", nargs="*", default="missing")
    parser.add_argument("prefix", nargs="*")
    parser.add_argument("middle", nargs="?")
    parser.add_argument("tail", nargs=2)
    return parser.parse_args()


print(repr(read()))
