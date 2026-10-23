import sys
from argparse import ArgumentParser
from pathlib import Path


def first():
    parser = ArgumentParser(prog="first")
    parser.add_argument("--mode", choices=("small", "large"), required=True)
    return parser.parse_args()


def second():
    parser = ArgumentParser(prog="second")
    parser.add_argument("--mode", type=Path, required=True)
    parser.add_argument("--flag", action="store_true", default=None)
    parser.add_argument("--paths", type=Path, nargs="+")
    return parser.parse_args()


sys.argv.append("--mode")
sys.argv.append("small")
print(first().mode)
settings = second()
print(settings.mode.name)
print(repr(settings.flag))
print(repr(settings.paths))
