"""Compare Python optimization settings used by dispatch-wrapper metadata tests."""

import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile


PROGRAM = '''"""Module documentation."""
import sys
from sys import flags

def evaluated() -> bool:
    print("assert condition evaluated")
    return False

def local_doc(__doc__: str) -> str:
    return __doc__

print(sys.flags.optimize)
print(flags.optimize)
print(__debug__)
print(__doc__)
print(local_doc("explicit binding"))
try:
    assert evaluated(), "failed assertion"
except AssertionError as error:
    print(str(error))
print("done")
'''


def output(command, env, root):
    result = subprocess.run(command, env=env, cwd=root, capture_output=True, timeout=60)
    if result.returncode:
        raise RuntimeError(f"{command[:2]}: {result.stderr.decode()}")
    return result.stdout, result.stderr


def check_assertion_bindings(compiler, env, root):
    bodies = [
        "assert (x := 2)\n    return x",
        "assert (x := 2)\n    return 1 + x",
        "assert [(x := item) for item in range(2)]\n    return x",
        "assert (lambda y=(x := 2): y)\n    return x",
        "assert (lambda: (x := 2))\n    return x",
        "assert [x for x in range(2)]\n    return x",
        "assert (x := 2)\n    def inner() -> int:\n        return x\n    return inner()",
        "assert (x := 2)\n    return [x for i in range(2)][0]",
        "value = 9\n    assert (value := 2)\n    return value",
    ]
    for level in ("-O", "-OO"):
        for body in bodies:
            program = ("x = 41\ndef read() -> int:\n    " + body
                       + "\ntry:\n    print(read())"
                       + "\nexcept UnboundLocalError as e:\n    print('local: ' + str(e))"
                       + "\nexcept NameError as e:\n    print('free: ' + str(e))")
            args = [level, "-c", program]
            if output([compiler, *args], env, root) != output([sys.executable, *args], env, root):
                raise AssertionError((level, body))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with the CPython 3.14.5 reference interpreter")
    compiler = str(args.imre.resolve())
    flags = ["--opt-level", args.opt_level, "--clang", args.clang,
             "--runtime", str(args.runtime.resolve()), "--no-cache"]
    if args.opt_level == "0":
        flags.append("--sanitize")
    env = dict(os.environ, IMRE_ENCODED_FLAGS="\x1f".join(flags))
    env.pop("PYTHONOPTIMIZE", None)
    cases = [
        ([], None), (["-O"], None), (["-OO"], None), (["-O", "-O"], None),
        (["-OOO"], None), ([], ""), ([], "0"), (["-O"], "3"), (["-OO"], "0"),
        ([], "-1"), ([], "invalid"), ([], "\t2"), ([], "2 "), ([], "2147483648"),
        (["-I"], "3"), (["-I", "-O"], "3"),
    ]
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        for options, setting in cases:
            case_env = env if setting is None else dict(env, PYTHONOPTIMIZE=setting)
            arguments = [*options, "-c", PROGRAM]
            expected = output([sys.executable, *arguments], case_env, root)
            actual = output([compiler, *arguments], case_env, root)
            if actual != expected:
                raise AssertionError((options, setting, actual, expected))

        # Optimized assertions must not require any code for the condition or message.
        arguments = ["-O", "-c", "assert missing_condition(), missing_message()\nprint('kept')"]
        if output([compiler, *arguments], env, root) != output([sys.executable, *arguments], env, root):
            raise AssertionError("the discarded assertion was evaluated")
        live = subprocess.run([compiler, "-O", "-c",
                               "assert missing_condition()\nprint(missing_live)"],
                              env=env, cwd=root, capture_output=True, timeout=60)
        if live.returncode == 0 or b"<string>.py:2:" not in live.stderr:
            raise AssertionError(("a diagnostic outside the assertion was lost", live.stderr))

        # The assertion is never evaluated, but its forbidden bindings are syntax errors.
        for source in (
            "assert (__debug__ := False)",
            "assert True, (__debug__ := False)",
            "assert [0 for __debug__ in ()]",
            "assert (lambda __debug__: True)",
            "assert [0 for obj.__debug__ in ()]",
            "assert (lambda x=(__debug__ := False): x)",
            "def unused() -> None:\n    assert (lambda: (__debug__ := True))",
        ):
            for level in ("-O", "-OO"):
                for executable in (sys.executable, compiler):
                    proc = subprocess.run([executable, level, "-c", source + "\nprint('continued')"],
                                          env=env, cwd=root, capture_output=True, timeout=60)
                    if not proc.returncode or proc.stdout or b"cannot assign to __debug__" not in proc.stderr:
                        raise AssertionError((source, level, proc.stdout, proc.stderr))

        # A file build chooses LLVM optimization independently of Python debug semantics.
        script = root / "program.py"
        binary = root / "program"
        script.write_text(PROGRAM, encoding="utf-8")
        output([compiler, "build", script, *flags, "-o", binary], env, root)
        expected = output([sys.executable, "-E", script], env, root)
        if output([binary], dict(env, PYTHONOPTIMIZE="2"), root) != expected:
            raise AssertionError("the native executable changed Python settings at runtime")
        check_assertion_bindings(compiler, env, root)
    print(f"{len(cases) + 1} startup commands, a file build, and assertion scopes matched; reserved assertion targets were rejected at LLVM -O{args.opt_level}")


if __name__ == "__main__":
    main()
