"""Run vendored CPython methods with a native child and host unittest/pipe machinery."""
import argparse
import ast
from functools import cache
import os
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
    parser.add_argument("--imre", required=True, type=Path)
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
    with tempfile.TemporaryDirectory(prefix="imre-sys-") as tmp:
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
            subprocess.run([args.imre.resolve(), "build", script, *flags, "-o", exe], check=True)
            return exe

        if Path("/dev/full").exists():
            code = """import sys
try:
    sys.stderr.write("probe\\n")
    sys.stderr.flush()
except OSError as error:
    print(str(error))
"""
            with open("/dev/full", "wb") as sink:
                messages = [subprocess.run(cmd, stdout=subprocess.PIPE, stderr=sink).stdout
                            for cmd in ([args.python, "-c", code], [native(code)])]
            if not messages[0] or messages[0] != messages[1]:
                raise AssertionError(("stderr write error", messages))

            recovery = Path(__file__).parent / "full/sys/stream_errno.py"
            executable = native(recovery.read_text())
            for mode in ("stdout-write", "stderr-write", "stdout-flush", "stderr-flush", "stdout-displayhook"):
                stock, actual = [subprocess.run([*cmd, mode], capture_output=True, check=True, timeout=10)
                                 for cmd in ([args.python, recovery], [executable])]
                if (actual.stdout, actual.stderr) != (stock.stdout, stock.stderr):
                    raise AssertionError((mode, stock, actual))
            print("Matched errno, strerror and recovery for stream and display hook failures")

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
        code = 'import sys\nsys.stdout.write("out")\nsys.stderr.write("err")'
        executable = native(code)
        output = [subprocess.check_output(cmd, stderr=subprocess.STDOUT)
                  for cmd in ([args.python, executable.with_suffix(".py")], [executable])]
        if output[0] != output[1]:
            raise AssertionError(("final stream order", output))

        if Path("/dev/full").exists():
            cases = [
                ('import sys\nsys.stdout.write("buffered")', "stdout"),
                ('import sys\nsys.stdout.write("buffered")\nsys.exit(7)', "stdout"),
                ('import sys\nsys.exit("message")', "stderr"),
                ('raise ValueError("bad")', "stderr"),
            ]
            for code, stream in cases:
                executable = native(code)
                with open("/dev/full", "wb") as sink:
                    pipes = {"stdout": subprocess.PIPE, "stderr": subprocess.PIPE, stream: sink}
                    results = [subprocess.run(cmd, **pipes)
                               for cmd in ([args.python, executable.with_suffix(".py")], [executable])]
                expected, actual = [(p.returncode, p.stdout, p.stderr) for p in results]
                if expected[0] != 120 or actual != expected:
                    raise AssertionError(("final stream flush", stream, expected, actual))

        print(f"Matched exit status, stdout, and stderr in {len(modes)} native sys fixture cases")

        payloads = Path(__file__).parent / "full/sys/exit_payloads.py"
        program = native(payloads.read_text())
        modes = ("empty-call", "singleton", "singleton-none", "tuple", "nested",
                 "constructor", "constructor-empty", "constructor-pair", "extra", "keyword")
        for mode in modes:
            for action in ("catch", "uncaught"):
                outputs = [subprocess.run([*command, action, mode], capture_output=True, timeout=10)
                           for command in ([args.python, payloads], [program])]
                # Native traceback frames are a separate open introspection item.
                # For an uncaught TypeError compare the final exception and both other outputs.
                def observed(p):
                    err = p.stderr
                    if action == "uncaught" and mode in ("extra", "keyword"):
                        err = err.splitlines()[-1:]
                    return p.returncode, p.stdout, err
                if observed(outputs[0]) != observed(outputs[1]):
                    raise AssertionError((mode, action, outputs))
        print("Matched 20 caught and uncaught SystemExit payload observations")

        if Path("/dev/full").exists():
            source = Path(__file__).parent / "full/sys/stream_buffer.py"
            program = native(source.read_text())
            writes = [(n,) for n in ("0", "1", "8191", "8192", "8193", "17000", "131071", "131072", "131073")]
            writes += [("65536", "65536"), ("65536", "65535", "1", "1"), ("1", "131071"), ("2", "131071"),
                       ("8191", "122881"), ("131071", "1"), ("131071", "2"), ("131000", "71", "2"),
                       ("8191", "1", "122880"), ("65536", "65537"), ("u65536",), ("u65535", "1"),
                       ("3", "LF"), ("3", "CR"), ("17000", "LF")]
            for fd in ("1", "2"):
                for parts in writes:
                    stock, actual = [subprocess.run([*cmd, fd, *parts], capture_output=True, check=True, timeout=10)
                                     for cmd in ([args.python, source], [program])]
                    if (stock.stdout, stock.stderr) != (actual.stdout, actual.stderr):
                        def state(run):
                            data, stage = (run.stdout, run.stderr) if fd == "1" else (run.stderr, run.stdout)
                            return stage, len(data)
                        raise AssertionError((fd, parts, state(stock), state(actual)))
            print(f"Matched the failed operation and retained bytes for {2 * len(writes)} standard stream sequences")

            # stdout starts on a terminal; the same child then replaces fd 1 with /dev/full.
            master, slave = os.openpty()
            try:
                for newline in ("LF", "CR"):
                    phases = [subprocess.run([*cmd, "1", "3", newline], stdout=slave, stderr=subprocess.PIPE,
                                             check=True, timeout=10).stderr for cmd in ([args.python, source], [program])]
                    if phases != [b"write 2\n", b"write 2\n"]:
                        raise AssertionError(("terminal at startup", newline, phases))
            finally:
                os.close(slave)
                os.close(master)


if __name__ == "__main__":
    main()
