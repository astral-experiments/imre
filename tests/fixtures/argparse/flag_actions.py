import argparse


def parse():
    parser = argparse.ArgumentParser(prog="flags")
    parser.add_argument("-v", "--verbose", action="count", help="number (%(default)s, %(nargs)s)")
    parser.add_argument("-q", action="count", default=999999999999999999999999999999)
    parser.add_argument("-f", "--off", action="store_false", help="switch to %(const)s")
    parser.add_argument("-s", action="store_const", const="yes", default=7)
    parser.add_argument("-n", action="store_const", default="old")
    parser.add_argument("--pi", action="store_const", const=3.5)
    parser.add_argument("--toggle", action="store_const", const=False)
    return parser.parse_args()


print(repr(parse()))
