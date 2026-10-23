"""
Read named inputs.

Examples:
    %(prog)s --items red green
"""

from argparse import ArgumentParser as Parser
from argparse import RawDescriptionHelpFormatter as Raw
from pathlib import Path as P
from typing import Literal


def read_options():
    parser = Parser(description=__doc__, formatter_class=Raw)
    parser.add_argument("--items", nargs="+", required=True, help="The values to read")
    parser.add_argument("--item-kind", choices=["green", "long-blue"], default="green")
    parser.add_argument(
        "--input-path", type=P, default=P("./work"), help="Root (default: %(default)s)"
    )
    parser.add_argument("--output", type=P, help="Output path")
    parser.add_argument(
        "--label", type=str, help="Label with two-line help and 100%% accuracy"
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Print long human-readable messages"
    )
    parser.add_argument("--debug", action="store_true", default=None)
    parser.add_argument("--locations", type=P, nargs="+")
    opts = parser.parse_args()
    return opts


def select(value: Literal["green", "long-blue"]) -> str:
    return value


def run() -> None:
    try:
        opts = read_options()
    except Exception:
        print("ordinary exception")
        raise
    except SystemExit as error:
        print("exit=" + str(error))
        raise
    finally:
        print("finished")
    print(repr(opts))
    print(opts == read_options())
    print(repr(opts.items))
    print(select(opts.item_kind))
    print(str(opts.input_path))
    print(repr(opts.label))
    print(opts.verbose)
    if opts.output:
        print(opts.output.name)


run()
