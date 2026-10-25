"""CPython 3.14.5's original str assertions with native splitlines and isspace results."""
import argparse
import ast
import hashlib
import inspect
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from test import support
from unittest.mock import patch

# requires_resource('cpu') records this setting while importing the test module.
support.use_resources = ["cpu"]
from test import test_str

StrTest = test_str.StrTest
METHODS = ("test_splitlines", "test_isspace")


def original(dispatch):
    class Text(str):
        def splitlines(self, *args, **kwargs):
            return dispatch("splitlines", str(self), args, kwargs)

        def isspace(self, *args, **kwargs):
            return dispatch("isspace", str(self), args, kwargs)

    class Case(StrTest):
        type2test = Text

        def checkequalnofix(self, result, obj, name, *args):
            # Preserve the original helper's type and equality assertions.
            return super().checkequalnofix(result, Text(obj), name, *args)

    return unittest.TestSuite(Case(name) for name in METHODS).run(unittest.TestResult())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    # CPython v3.14.5 Lib/test/{string_tests,test_str}.py, under the PSF license.
    for method, digest in (
        (StrTest.test_splitlines, "202b1962796b6a546f4027f9d08ec948400683c13886dbfd072d969545f8f67f"),
        (super(StrTest, StrTest).test_isspace, "1baae9886fd88d0328a08474e12e482f45d1aade82b4c09fdb07300f25161fec"),
        (StrTest.test_isspace, "a3e37ce2027882461b5d8c71627f04e96be998a09618183f1c1020b8c67a4e3b"),
        (StrTest.test_isspace_invariant, "9776560d288bc1e931462d8d267f9ee75ac6c036125592fd7e32bf9b3402f143"),
    ):
        assert hashlib.sha256(inspect.getsource(method).encode()).hexdigest() == digest

    queries = []
    def record(name, text, args, kwargs):
        queries.append((name, text, args, kwargs))
        return getattr(text, name)(*args, **kwargs)
    assert original(record).wasSuccessful()
    assert sum(name == "splitlines" for name, *_ in queries) == 11
    assert sum(name == "isspace" for name, *_ in queries) == 12
    with tempfile.TemporaryDirectory() as tmp:
        program = Path(tmp) / "lines.py"
        binary = Path(tmp) / "lines"
        calls = []
        for name, text, positional, keywords in queries:
            parts = [repr(arg) for arg in positional] + [f"{k}={v!r}" for k, v in keywords.items()]
            calls.append(f"try:\n    print({text!r}.{name}({', '.join(parts)}))\n"
                         'except TypeError as err:\n    print("TypeError:" + str(err))\n')
        program.write_text("".join(calls) + """
count = 0
for value in range(0x110000):
    if value < 0xD800 or value > 0xDFFF:
        count += 1
        if chr(value).isspace():
            print(value)
print(count)
""")
        for mode in ([], ["--reachable-only"]):
            for level in ("0", "3"):
                subprocess.run([args.compiler, "build", program, "--runtime", args.runtime,
                                "--clang", args.clang, "--opt-level", level, "-o", binary, *mode],
                               check=True, capture_output=True)
                run = subprocess.run([binary], check=True, capture_output=True, text=True)
                rows = run.stdout.splitlines()
                assert not run.stderr and int(rows[-1]) == 0x110000 - 0x800
                answers = rows[:len(queries)]
                positive = {int(item) for item in rows[len(queries):-1]}

                def replay(values):
                    items = iter(zip(queries, values, strict=True))
                    def lookup(name, text, args, kwargs):
                        query, answer = next(items)
                        assert (name, text, args, kwargs) == query
                        if answer.startswith("TypeError:"):
                            raise TypeError(answer.removeprefix("TypeError:"))
                        return ast.literal_eval(answer)
                    return original(lookup)
                checked = replay(answers)
                assert checked.wasSuccessful(), (checked.errors, checked.failures)
                # A wrong native split must fail an unchanged assertion.
                assert replay(["[]", *answers[1:]]).failures

                class Scalar(str):
                    def isspace(self):
                        cp = ord(self)
                        if 0xD800 <= cp <= 0xDFFF:
                            return super().isspace()  # Native strings contain UTF-8 scalars.
                        return cp in positive
                with patch.object(test_str, "chr", lambda cp: Scalar(chr(cp)), create=True):
                    invariant = StrTest("test_isspace_invariant").run()
                    assert invariant.wasSuccessful() and not invariant.skipped
                assert len(positive) == 29
                label = "reachable" if mode else "whole"
                print(f"{label} O{level}: 11 native splitlines; 12 native + 6 host literal isspace; "
                      "1,112,064 native scalar + 2,048 host surrogate invariant checks")


if __name__ == "__main__":
    main()
