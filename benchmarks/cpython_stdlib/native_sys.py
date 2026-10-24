"""Run vendored CPython methods with a native child and host unittest/pipe machinery."""
import argparse
import ast
from functools import cache
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import upstream

CASES = {
    "ProcessTestCase": ("test_communicate_stdout", "test_communicate_stderr"),
    "RunFuncTestCase": ("test_returncode", "test_check"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relar", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-level", default="3")
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()

    upstream.verify_upstream()
    source = Path(upstream.__file__).parent / "upstream/Lib/test/test_subprocess.py"
    data = source.read_text()
    classes = {node.name: node for node in ast.parse(data).body if isinstance(node, ast.ClassDef)}
    with tempfile.TemporaryDirectory(prefix="relar-sys-") as tmp:
        folder = Path(tmp)
        flags = ["--runtime", args.runtime.resolve(), "--clang", args.clang,
                 "--opt-level", args.opt_level, "--quiet"]
        if args.sanitize:
            flags.append("--sanitize")

        @cache
        def native(code):
            index = native.cache_info().misses
            script, exe = folder / f"child{index}.py", folder / f"child{index}"
            script.write_text(code + "\n")
            subprocess.run([args.relar.resolve(), "build", script, *flags, "-o", exe], check=True)
            return exe

        def command(argv):
            if len(argv) != 3 or argv[:2] != [sys.executable, "-c"]:
                raise ValueError(f"unexpected upstream child invocation: {argv!r}")
            return [native(argv[2])]

        process = SimpleNamespace(
            PIPE=subprocess.PIPE, CalledProcessError=subprocess.CalledProcessError,
            Popen=lambda argv, **kwargs: subprocess.Popen(command(argv), **kwargs),
            run=lambda argv, **kwargs: subprocess.run(command(argv), **kwargs),
        )
        suite = unittest.TestSuite()
        for cls, names in CASES.items():
            block = classes[cls]
            helpers = ("run_python",) if cls == "RunFuncTestCase" else ()
            methods = {}
            for name in (*names, *helpers):
                method = next(n for n in block.body if isinstance(n, ast.FunctionDef) and n.name == name)
                assert not method.decorator_list
                body = textwrap.dedent(ast.get_source_segment(data, method))
                ns = {"sys": sys, "subprocess": process}
                exec(compile("\n" * (method.lineno - 1) + body, str(source), "exec"), ns)
                methods[name] = ns[name]
            case_type = type(cls, (unittest.TestCase,), {"__module__": "test.test_subprocess", **methods})
            suite.addTests(case_type(name) for name in names)
        if not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful():
            raise SystemExit(1)

        fixture = Path(__file__).resolve().parents[2] / "tests/fixtures/sys/streams_exit.py"
        program = native(fixture.read_text())
        modes = ("stdio", "catch", "none", "explicit", "number", "huge", "text", "empty",
                 "bool", "raised", "class", "unicode", "negative")
        for mode in modes:
            outputs = [subprocess.run([*command, mode], capture_output=True, timeout=10)
                       for command in ([args.python, fixture], [program])]
            result = [(p.returncode, p.stdout, p.stderr) for p in outputs]
            if result[0] != result[1]:
                raise AssertionError((mode, result))
        print(f"Matched exit status, stdout, and stderr in {len(modes)} native sys fixture cases")


if __name__ == "__main__":
    main()
