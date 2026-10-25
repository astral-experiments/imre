"""Run vendored CPython unittest methods with native Imre subprocess.run calls."""
import argparse
import ast
from functools import cache
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import upstream

METHODS = ("test_capture_stdout", "test_capture_stderr", "test_capture_output", "test_check",
           "test_check_zero", "test_timeout", "test_check_output_timeout", "test_stdout_stdout")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-level", default="3")
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    upstream.verify_upstream()
    path = Path(upstream.__file__).parent / "upstream/Lib/test/test_subprocess.py"
    tree = ast.parse(path.read_text())
    case = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "RunFuncTestCase")

    with tempfile.TemporaryDirectory(prefix="imre-process-") as tmp:
        @cache
        def native(options):
            # Each literal option fixes both the result's type and its native pipe setup.
            name = str(native.cache_info().misses)
            source, exe = Path(tmp) / (name + ".py"), Path(tmp) / name
            source.write_text(f"""import subprocess
import sys
try:
    result = subprocess.run(sys.argv[1:]{options})
    print("ok")
    print(result.returncode)
    print(repr(result.stdout))
    print(repr(result.stderr))
except subprocess.CalledProcessError as error:
    print("checked")
    print(error.returncode)
    print(repr(error.output))
    print(repr(error.stderr))
except subprocess.TimeoutExpired as error:
    print("timeout")
    print(repr(error.output))
    print(repr(error.stderr))
except ValueError as error:
    print("invalid")
    print(str(error))
""")
            flags = ["--sanitize"] if args.sanitize else []
            subprocess.run([args.imre.resolve(), "build", source, "--quiet", "--runtime",
                            args.runtime.resolve(), "--clang", args.clang, "--opt-level",
                            args.opt_level, *flags, "-o", exe], check=True)
            return exe

        def run(argv, **kwargs):
            flags = []
            streams = {subprocess.PIPE: "subprocess.PIPE", subprocess.STDOUT: "subprocess.STDOUT"}
            for name, value in kwargs.items():
                literal = streams[value] if name in ("stdout", "stderr") else repr(value)
                flags.append(f", {name}={literal}")
            child = subprocess.run([native("".join(flags)), *argv], capture_output=True,
                                   text=True, encoding="utf-8", timeout=15, check=True)
            if child.stderr:
                raise RuntimeError(f"unexpected native stderr: {child.stderr}")
            status, *values = child.stdout.splitlines()
            if status == "invalid":
                raise ValueError(values[0])
            if status == "timeout":
                stdout, stderr = map(ast.literal_eval, values)
                raise subprocess.TimeoutExpired(argv, kwargs["timeout"], output=stdout, stderr=stderr)
            returncode, stdout, stderr = map(ast.literal_eval, values)
            if status == "checked":
                raise subprocess.CalledProcessError(returncode, argv, output=stdout, stderr=stderr)
            if status != "ok":
                raise RuntimeError(f"unexpected native result: {child.stdout}")
            return subprocess.CompletedProcess(argv, returncode, stdout, stderr)

        # Keep the original method and helper ASTs, including assertions and their
        # line numbers. Host unittest supplies assertion contexts; run() delegates
        # launch, capture, checked status, and timeouts to the native parent.
        proxy = SimpleNamespace(run=run, PIPE=subprocess.PIPE, STDOUT=subprocess.STDOUT,
                                CalledProcessError=subprocess.CalledProcessError,
                                TimeoutExpired=subprocess.TimeoutExpired)
        namespace = {"sys": sys, "subprocess": proxy}
        zero = next(n for n in tree.body if isinstance(n, ast.Assign)
                    and isinstance(n.targets[0], ast.Name) and n.targets[0].id == "ZERO_RETURN_CMD")
        methods = [n for n in case.body if isinstance(n, ast.FunctionDef)
                   and n.name in (*METHODS, "run_python")]
        assert len(methods) == len(METHODS) + 1 and not any(n.decorator_list for n in methods)
        exec(compile(ast.Module([zero, *methods], []), str(path), "exec"), namespace)
        case_type = type(case.name, (unittest.TestCase,),
                         {"__module__": "test.test_subprocess", **{n.name: namespace[n.name] for n in methods}})
        suite = unittest.TestSuite(case_type(name) for name in METHODS)
        if not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful():
            raise SystemExit(1)

        fixture = Path(__file__).resolve().parents[2] / "tests/fixtures/process_streams/modes.py"
        executable = Path(tmp) / "modes"
        subprocess.run([args.imre.resolve(), "build", fixture, "--quiet", "--runtime",
                        args.runtime.resolve(), "--clang", args.clang, "--opt-level", args.opt_level,
                        *(["--sanitize"] if args.sanitize else []), "-o", executable], check=True)
        results = [subprocess.run([*cmd, sys.executable], capture_output=True, timeout=15)
                   for cmd in ([sys.executable, fixture], [executable])]
        expected, actual = [(run.returncode, run.stdout, run.stderr) for run in results]
        if expected[0] or expected != actual:
            raise AssertionError((expected, actual))
        print("Matched the mixed-stream fixture's return code and both output descriptors.")


if __name__ == "__main__":
    main()
