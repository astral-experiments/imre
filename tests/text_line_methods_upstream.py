"""CPython 3.14.5's str assertions with native line methods and Unicode predicate results."""
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
sys.path.insert(0, str(Path(__file__).resolve().parent))
from importlib_alnum_sites import cache_guards

# requires_resource('cpu') records this setting while importing the test module.
support.use_resources = ["cpu"]
from test import test_str

StrTest = test_str.StrTest
METHODS = ("test_splitlines", "test_isspace", "test_isupper", "test_isalnum")


def original(dispatch):
    class Text(str):
        def splitlines(self, *args, **kwargs):
            return dispatch("splitlines", str(self), args, kwargs)

        def isspace(self, *args, **kwargs):
            return dispatch("isspace", str(self), args, kwargs)

        def isupper(self, *args, **kwargs):
            return dispatch("isupper", str(self), args, kwargs)

        def isalnum(self, *args, **kwargs):
            return dispatch("isalnum", str(self), args, kwargs)

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
    suite = Path(__file__).resolve().parents[1] / "benchmarks/cpython_stdlib/full"
    assert (suite / "str/upstream/Lib/test/test_str.py").read_bytes() == Path(test_str.__file__).read_bytes()
    assert ((suite / "collections/upstream/Lib/test/string_tests.py").read_bytes()
            == Path(test_str.string_tests.__file__).read_bytes())
    # CPython v3.14.5 Lib/test/{string_tests,test_str}.py, under the PSF license.
    for method, digest in (
        (StrTest.test_splitlines, "202b1962796b6a546f4027f9d08ec948400683c13886dbfd072d969545f8f67f"),
        (super(StrTest, StrTest).test_isspace, "1baae9886fd88d0328a08474e12e482f45d1aade82b4c09fdb07300f25161fec"),
        (StrTest.test_isspace, "a3e37ce2027882461b5d8c71627f04e96be998a09618183f1c1020b8c67a4e3b"),
        (StrTest.test_isspace_invariant, "9776560d288bc1e931462d8d267f9ee75ac6c036125592fd7e32bf9b3402f143"),
        (super(StrTest, StrTest).test_isupper, "55b206228a79e15d5e49d6b84e895820288c6f3251532b461d3a2642c5f0e016"),
        (StrTest.test_isupper, "986f6864830baeb329654321243dbccd37e35992d6a59dfd26cbe616fc41ead2"),
        (super(StrTest, StrTest).test_isalnum, "9bf86fb858da5e408187e740fb058460fc263294cc7d7efd56029b8aec3e3d0d"),
        (StrTest.test_isalnum, "12088c04532bdd422a1a5979249045cf48c48de06773b421b7766b7ebc31af45"),
    ):
        assert hashlib.sha256(inspect.getsource(method).encode()).hexdigest() == digest

    queries = []
    def record(name, text, args, kwargs):
        queries.append((name, text, args, kwargs))
        return getattr(text, name)(*args, **kwargs)
    assert original(record).wasSuccessful()
    assert sum(name == "splitlines" for name, *_ in queries) == 11
    assert sum(name == "isspace" for name, *_ in queries) == 12
    assert sum(name == "isupper" for name, *_ in queries) == 9
    assert sum(name == "isalnum" for name, *_ in queries) == 9
    # One uppercase prefix distinguishes uncased scalars from nonuppercase cased ones.
    upper = {i for i in range(0x110000) if chr(i).isupper()}
    blocks_upper = {i for i in range(0x110000) if not ("A" + chr(i)).isupper()}
    alnum = {i for i in range(0x110000) if chr(i).isalnum()}
    guards, cache_answers = cache_guards()
    with tempfile.TemporaryDirectory() as tmp:
        program = Path(tmp) / "lines.py"
        binary = Path(tmp) / "lines"
        calls = []
        for name, text, positional, keywords in queries:
            parts = [repr(arg) for arg in positional] + [f"{k}={v!r}" for k, v in keywords.items()]
            calls.append(f"try:\n    print({text!r}.{name}({', '.join(parts)}))\n"
                         'except TypeError as err:\n    print("TypeError:" + str(err))\n')
        program.write_text(guards + "".join(calls) + """
count = 0
for value in range(0x110000):
    if value < 0xD800 or value > 0xDFFF:
        count += 1
        ch = chr(value)
        flags = 0
        if ch.isspace():
            flags += 1
        if ch.isupper():
            flags += 2
        if not ("A" + ch).isupper():
            flags += 4
        if ch.isalnum():
            flags += 8
        if flags:
            print(value * 16 + flags)
print(count)
""")
        for mode in ([], ["--reachable-only"]):
            for level in ("0", "3"):
                compile = subprocess.run([args.compiler, "build", program, "--runtime", args.runtime,
                                          "--clang", args.clang, "--opt-level", level, "-o", binary, *mode],
                                         capture_output=True, text=True)
                assert compile.returncode == 0, compile.stderr
                run = subprocess.run([binary], check=True, capture_output=True, text=True)
                rows = run.stdout.splitlines()
                assert not run.stderr and int(rows[-1]) == 0x110000 - 0x800
                start = len(queries) + len(cache_answers)
                assert rows[:len(cache_answers)] == cache_answers
                answers = rows[len(cache_answers):start]
                classes = [divmod(int(item), 16) for item in rows[start:-1]]
                assert {i for i, flags in classes if flags & 8} == alnum
                positive = {i for i, flags in classes if flags & 1}
                assert {i for i, flags in classes if flags & 2} == upper
                assert {i for i, flags in classes if flags & 4} == blocks_upper

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
                for predicate in ("isupper", "isalnum"):
                    wrong = answers.copy()
                    index = next(i for i, q in enumerate(queries) if q[0] == predicate)
                    wrong[index] = str(not ast.literal_eval(wrong[index]))
                    assert replay(wrong).failures

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
                      "9 native + 8 host literal isupper; 9 native + 8 host literal isalnum; "
                      "1,112,064 native scalars + "
                      f"2,048 host surrogate whitespace checks; {len(cache_answers)} CPython cache-guard outcomes")


if __name__ == "__main__":
    main()
