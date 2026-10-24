import sys
from argparse import ArgumentParser
from pathlib import Path


def first():
    parser = ArgumentParser(prog="first", description="\x1cChoose\va mode\x1f")
    parser.add_argument(
        "--mode", choices=("small", "large"), required=True, help="Select\vthe mode"
    )
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
