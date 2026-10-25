from argparse import ArgumentParser
from pathlib import Path


def main() -> None:
    p = ArgumentParser()
    p.add_argument("-v", "--verbose", action="store_true")
    p.add_argument("names", type=Path, nargs="+")
    args = p.parse_args()
    print(args.verbose)
    for name in args.names:
        print(name)


main()
