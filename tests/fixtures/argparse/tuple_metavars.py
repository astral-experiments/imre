import argparse
import sys


def grouped():
    parser = argparse.ArgumentParser(prog='PROG')
    g = parser.add_mutually_exclusive_group()
    g.add_argument('--op1', metavar='MET', nargs='?')
    g.add_argument('--op2', metavar=('MET1', 'MET2'), nargs='*')
    g.add_argument('--op3', nargs='*')
    g.add_argument('--op4', metavar=('MET1', 'MET2'), nargs='+')
    g.add_argument('--op5', nargs='+')
    g.add_argument('--op6', nargs=3)
    g.add_argument('--op7', metavar=('MET1', 'MET2', 'MET3'), nargs=3)
    return parser.parse_args()


def labels(words: list[str]):
    parser = argparse.ArgumentParser(prog="labels")
    parser.add_argument("--one", choices=["x", "y"], metavar=("O'Reilly",), help="%(dest)s: %(metavar)s")
    parser.add_argument("--many", nargs="*", metavar=("", "雪"), help="%(metavar)s")
    return parser.parse_args(words)


if len(sys.argv) > 1 and sys.argv[1] == "labels":
    print(repr(labels(sys.argv[2:])))
else:
    print(repr(grouped()))
