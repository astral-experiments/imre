"""Run CPython 3.14.5's str.expandtabs assertions, preserving their native/host roles."""
import argparse
import ast
import hashlib
import inspect
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from test.test_str import StrTest


def compare_controls(args, mode, level, binary, relative, limit=None):
    fixture = Path(__file__).resolve().parent / "fixtures" / relative
    subprocess.run([args.compiler, "build", fixture, "--runtime", args.runtime, "--clang",
                    args.clang, "--opt-level", level, "-o", binary, *mode], check=True,
                   capture_output=True)
    def run(command):
        return subprocess.run(command, check=True, capture_output=True, preexec_fn=limit)
    oracle = run([sys.executable, "-I", "-B", str(fixture)])
    actual = run([str(binary)])
    assert not oracle.stderr and (actual.stdout, actual.stderr) == (oracle.stdout, oracle.stderr)
    if limit:
        assert oracle.stdout == b"MemoryError()\n''\n'xx  '\n"


def original(expand):
    class Text(str):
        def expandtabs(self, *args, **kwargs):
            return expand(str(self), args, kwargs)

    class Case(StrTest):
        type2test = Text

    return Case("test_expandtabs").run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    hashes = {
        "test_expandtabs": "a0b7715ca5a740eeda649186f373330dac59dc53bc8a0b070be700820dea7b7b",
        "test_expandtabs_optimization": "a54106c42fb46e05feaf14efca00e11d989dcee08a136cd7fbc438b029cf657d",
        "test_expandtabs_overflows_gracefully": "83f15c32f36f41c713393d8dd87977a4ff2ab265eebdb7cdd80191065724fe2a",
    }
    for name, expected in hashes.items():
        source = inspect.getsource(getattr(StrTest, name)).encode()
        assert hashlib.sha256(source).hexdigest() == expected
    # Do not count the original 32-bit-only overflow case on a 64-bit target.
    platform_case = StrTest("test_expandtabs_overflows_gracefully").run()
    assert platform_case.wasSuccessful()
    is_32bit = sys.maxsize < 1 << 32 and struct.calcsize("P") == 4
    assert bool(platform_case.skipped) != is_32bit

    queries = []
    def record(text, positional, keywords):
        queries.append((text, positional, keywords))
        return text.expandtabs(*positional, **keywords)
    assert original(record).wasSuccessful()
    assert len(queries) == 11 + is_32bit
    with tempfile.TemporaryDirectory() as tmp:
        # The default never calls a source __index__, including in reachable-only mode.
        unused = Path(tmp) / "unused_index.py"
        unused.write_text('class Cold:\n    def __index__(self) -> int:\n'
                          '        return eval("42")\nCold()\nprint("a\\tb".expandtabs())\n')
        whole = subprocess.run([args.compiler, "check", unused], capture_output=True)
        assert whole.returncode and b"unused_index.py:3:" in whole.stderr, whole.stderr
        subprocess.run([args.compiler, "check", unused, "--reachable-only"], check=True)
        source = Path(tmp) / "tabs.py"
        binary = Path(tmp) / "tabs"
        # Keep the identity test method byte-identical; its CPython-only decorator
        # performs host selection, and the native fixture supplies assertIs.
        method = inspect.getsource(StrTest.test_expandtabs_optimization)
        method = method[method.index("    def test_expandtabs_optimization"):]
        parts = ["class Case:\n    def assertIs(self, actual: str, expected: str) -> None:\n"
                 '        assert actual is expected\n        print("identity")\n\n',
                 method, "\nCase().test_expandtabs_optimization()\n"]
        for text, positional, keywords in queries:
            values = [repr(v) for v in positional] + [f"{k}={v!r}" for k, v in keywords.items()]
            parts.append(f"try:\n    print(repr({text!r}.expandtabs({', '.join(values)})))\n"
                         'except TypeError as e:\n    print("TypeError:" + str(e))\n'
                         'except OverflowError as e:\n    print("OverflowError:" + str(e))\n')
        source.write_text("".join(parts))
        for mode in ([], ["--reachable-only"]):
            for level in ("0", "3"):
                subprocess.run([args.compiler, "build", source, "--runtime", args.runtime, "--clang",
                                args.clang, "--opt-level", level, "-o", binary, *mode], check=True,
                               capture_output=True)
                run = subprocess.run([binary], check=True, capture_output=True, text=True)
                identity, *answers = run.stdout.splitlines()
                assert not run.stderr and identity == "identity" and len(answers) == len(queries)
                def replay(provided):
                    groups = iter(zip(queries, provided, strict=True))
                    def lookup(text, positional, keywords):
                        query, output = next(groups)
                        assert query == (text, positional, keywords)
                        for cls in TypeError, OverflowError:
                            if output.startswith(cls.__name__ + ":"):
                                raise cls(output.split(":", 1)[1])
                        return ast.literal_eval(output)
                    return original(lookup)
                checked = replay(answers)
                assert checked.wasSuccessful(), (checked.errors, checked.failures)
                assert replay([repr("incorrect native expansion"), *answers[1:]]).failures
                label = "reachable" if mode else "whole"
                print(f"{label} O{level}: one native identity method, {len(queries)} native queries "
                      f"under the original host assertions, 32-bit-only skip={not is_32bit}")
                compare_controls(args, mode, level, binary, "text/expandtabs.py")
                if sys.platform == "linux":
                    import resource
                    def limited():
                        resource.setrlimit(resource.RLIMIT_AS, (128 << 20, 128 << 20))
                    compare_controls(args, mode, level, binary, "text_limits/expandtabs.py", limited)


if __name__ == "__main__":
    main()
