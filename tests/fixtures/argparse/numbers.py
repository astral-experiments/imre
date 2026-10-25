from argparse import ArgumentParser


def read():
    parser = ArgumentParser(prog="numbers")
    parser.add_argument("-i", "--integer", type=int)
    parser.add_argument("--scale", type=float)
    parser.add_argument("--ids", type=int, nargs=2, choices=range(-2, 5, 2))
    parser.add_argument("--choice", type=float, choices=(-0.0, 2.5))
    parser.add_argument("--maybe", type=int, nargs="?", const="42", default="84", choices=[1, 2])
    parser.add_argument("--unit", choices="abc")
    parser.add_argument("ratio", type=float, nargs="?", default=0.25, help="default: %(default)s")
    return parser.parse_args()


print(repr(read()))
