"""Compare dispatcher docstrings and the native string assertion fixture with CPython."""

import argparse
import os
from pathlib import Path
import sys
import tempfile

from optimization_mode import output
from run_bodies import STRING_EXPECTATION


FAILURES = '''
def compare(first: str | None, second: str, message: str | None = None) -> None:
    try:
        NativeCase().assertEqual(first, second, message)
    except AssertionError as error:
        print(str(error))
        return
    print("equal")

def choose(calls: list[int], present: bool) -> str | None:
    calls.append(1)
    if present:
        return "'unquoted'"
    return None

compare(None, "Simple test")
compare(None, "repeated " * 20, "context")
compare("one\\ntwo", "one\\nthree", "different lines")
compare("same", "same")
calls: list[int] = []
print(choose(calls, True))
print(str(choose(calls, False)))
print(len(calls))
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--imre", type=Path, required=True)
    p.add_argument("--runtime", type=Path, required=True)
    p.add_argument("--clang", default="clang")
    p.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = p.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("run with CPython 3.14.5")
    flags = ["--opt-level", args.opt_level, "--clang", args.clang, "--runtime",
             str(args.runtime.resolve()), "--no-cache"]
    if args.opt_level == "0":
        flags.append("--sanitize")
    env = dict(os.environ, IMRE_ENCODED_FLAGS="\x1f".join(flags))
    compiler = str(args.imre.resolve())
    fixture = Path(__file__).resolve().parents[4] / "tests/fixtures/callables/single_dispatch.py"
    with tempfile.TemporaryDirectory() as tmp:
        for options in ([], ["-O"], ["-OO"]):
            command = ["-I", *options, "-c", fixture.read_text(encoding="utf-8")]
            actual = output([compiler, *command], env, tmp)
            expected = output([sys.executable, *command], env, tmp)
            if actual != expected:
                raise AssertionError((options, actual, expected))
            print(options, len(actual[0]), "output bytes matched")
        native = "import unittest\nclass NativeCase:\n" + STRING_EXPECTATION + FAILURES
        reference = "from unittest import TestCase as NativeCase\n" + FAILURES
        actual = output([compiler, "-I", "-c", native], env, tmp)
        expected = output([sys.executable, "-I", "-c", reference], env, tmp)
        if actual != expected:
            raise AssertionError(("native string assertion", actual, expected))
        print("assertion controls:", len(actual[0]), "output bytes matched")


if __name__ == "__main__":
    main()
