"""Run a persistent loopback task board: app PORT JOURNAL."""

import sys
from board import make_app
from httpwire import serve


def main() -> None:
    if len(sys.argv) != 3:
        raise ValueError("usage: app PORT JOURNAL")
    serve(make_app(sys.argv[2]), int(sys.argv[1]))


main()
