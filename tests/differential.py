"""Run the same source on CPython and Imre, including semantic edge cases."""

import argparse
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent


def run(command: list[str], stdin: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        input=stdin,
        text=True,
        capture_output=True,
        timeout=60,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
    )


def integer_input() -> str:
    boundaries = [0, 1, -1, 2**62 - 1, 2**62, -(2**62), -(2**62) - 1, 2**63, -(2**63), 2**127, -(2**127)]
    pairs = [(a, b) for a in boundaries for b in boundaries if b != 0]
    generator = random.Random(20261017)
    for _ in range(150):
        a = generator.getrandbits(generator.choice([32, 62, 64, 128, 512]))
        b = generator.getrandbits(generator.choice([16, 62, 128])) or 1
        pairs.append((a * generator.choice([-1, 1]), b * generator.choice([-1, 1])))
    return "\n".join([str(len(pairs)), *(str(value) for pair in pairs for value in pair)]) + "\n"


REJECTED = {
    "wrong_return": 'def f() -> int:\n    return "wrong"\nf()\n',
    "wrong_argument": 'def f(x: int) -> None:\n    pass\nf("wrong")\n',
    "wrong_list_write": 'def f(x: list[int]) -> None:\n    x[0] = "wrong"\nf([0])\n',
    "wrong_alias_write": 'def f(x: list[int]) -> None:\n    y = x\n    y.append("wrong")\nf([0])\n',
    "bool_is_not_exact_int": 'def f(x: list[int]) -> None:\n    x.append(True)\nf([0])\n',
    "ignored_error": 'def f() -> int:\n    return "wrong"  # type: ignore\nf()\n',
    "ty_ignored_error": 'def f() -> int:\n    return "wrong"  # ty: ignore[invalid-return-type]\nf()\n',
    "uninitialized": 'def f(flag: bool) -> int:\n    if flag:\n        x = 1\n    return x\nf(False)\n',
    "empty_loop": 'def f() -> int:\n    for x in range(0):\n        pass\n    return x\nf()\n',
    "rebind_type": 'def f() -> None:\n    x = 1\n    x = "wrong"\nf()\n',
    "rebind_builtin": 'def f() -> None:\n    len = 1\nf()\n',
    "dead_dynamic_code": 'def f() -> None:\n    if False:\n        eval("1 + 2")\nf()\n',
    "unknown_module": 'import missing_imre_test_module\nprint(1)\n',
    "recursive": 'def f(x: int) -> int:\n    return f(x)\nf(1)\n',
    "global_read": 'def f() -> int:\n    return x\nx = 1\nf()\n',
    "missing_return": 'def f(x: bool) -> int:\n    if x:\n        return 1\nf(False)\n',
    "inplace_list_repeat": 'def f() -> None:\n    x = [1]\n    x *= 2\nf()\n',
    "surrogate_literal": 'print("\\ud800")\n',
    "long_surrogate_literal": 'print("\\U0000dfff")\n',
    "surrogate_after_crlf": 'x = 1\r\nprint("\\ud800")\r\n',
    "source_null_literal": 'print("abc\0def")\n',
    "source_null_comment": 'print(1)  # \0\n',
    "latin1_cookie": '# coding: latin-1\nprint("é")\n',
    "second_line_cookie": '# comment\n# coding=latin-1\nprint("é")\n',
    "ascii_cookie": '# coding: ascii\nprint("é")\n',
    "tab_indentation": 'if True:\n\tif True:\n   print(1)\n',
    "form_feed_indentation": 'if True:\n \f print(1)\n',
    "normalized_none_binding": 'ℕone = 1\nprint(ℕone)\n',
    "normalized_true_binding": '𝑇rue = 1\nprint(𝑇rue)\n',
    "normalized_false_binding": '𝑭alse = 1\nprint(𝑭alse)\n',
    "surrogate_docstring": '"\\ud800"\nprint(1)\n',
}

SOURCE_BOUNDARIES = {
    "crlf_literal": 'x = """a\r\nb"""\r\nprint(len(x))\r\nprint(x == "a\\nb")\r\n',
    "cr_literal": 'x = """a\rb"""\rprint(len(x))\rprint(x == "a\\nb")\r',
    "raw_crlf_literal": 'x = r"""a\r\nb"""\nprint(len(x))\nprint(x == "a\\nb")\n',
    "escaped_cr_literal": 'x = "a\\rb"\r\nprint(len(x))\r\nprint(x == "a\\nb")\r\n',
    "utf8_cookie_bom": '\ufeff# coding: UTF_8\n# coding: latin1\nprint("é")\n',
    "ignored_second_cookie": 'print("é")\n# coding: latin1\n',
    "ignored_third_cookie": '# comment\n\n# coding: latin1\nprint("é")\n',
    "literal_indentation_data": 'x = """a\n\tb\n\fc"""\nprint(x == "a\\n\\tb\\n\\fc")\n',
}

ERRORS = {
    "division_zero": ("print(1 // 0)\n", "ZeroDivisionError"),
    "modulo_zero": ("print(-1 % 0)\n", "ZeroDivisionError"),
    "dynamic_division_zero": ('zero = int("0")\nprint(1 // zero)\n', "ZeroDivisionError"),
    "dynamic_modulo_zero": ('zero = int("0")\nprint(-1 % zero)\n', "ZeroDivisionError"),
    "modulo_zero_big": ("print(1000000000000000000000000 % 0)\n", "ZeroDivisionError"),
    "bounds": ("x = [1]\nprint(x[1])\n", "IndexError"),
    "negative_bounds": ("x = [1]\nprint(x[-2])\n", "IndexError"),
    "empty_negative_bounds": ("x: list[int] = []\nprint(x[-1])\n", "IndexError"),
    "minimum_machine_index": ("x = [1]\nprint(x[-9223372036854775808])\n", "IndexError"),
    "negative_big_bounds": ("x = [1]\nprint(x[-1000000000000000000000000])\n", "IndexError"),
    "big_bounds": ("x = [1]\nprint(x[1000000000000000000000000])\n", "IndexError"),
    "empty_pop": ("x: list[int] = []\nprint(x.pop())\n", "IndexError"),
    "bad_integer": ('print(int("12__3"))\n', "ValueError"),
    "empty_integer": ('print(int(""))\n', "ValueError"),
    "sign_only_integer": ('print(int("+"))\n', "ValueError"),
    "double_sign_integer": ('print(int("-+1"))\n', "ValueError"),
    "leading_separator_integer": ('print(int("+_1"))\n', "ValueError"),
    "trailing_separator_integer": ('print(int("1_"))\n', "ValueError"),
    "embedded_space_integer": ('print(int("1 2"))\n', "ValueError"),
    "control_separator_integer": ('print(int("\\x1c1"))\n', "ValueError"),
    "invalid_after_large_integer": (
        'print(int("999999999999999999999999999999999999999999999__1"))\n',
        "ValueError",
    ),
    "zero_step": ("for x in range(0, 1, 0):\n    print(x)\n", "ValueError"),
    "assertion": ("assert False\n", "AssertionError"),
    "repeat_overflow": ("x = [1] * -100000000000000000000000\n", "OverflowError"),
    "eof": ("print(input())\n", "EOFError"),
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    build_flags = []
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
    compiler = str(args.compiler.resolve())
    runtime = str(args.runtime.resolve())
    passed = 0
    with tempfile.TemporaryDirectory(prefix="imre-differential-") as directory:
        temporary = Path(directory)
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(source)
            checked = run([compiler, "check", str(path)])
            assert checked.returncode > 0 and checked.stderr, (name, checked)
            passed += 1
        cases = [(path.stem, path.read_text(), integer_input() if path.stem == "integer_matrix" else "", None)
                 for path in sorted((ROOT / "programs").glob("*.py"))]
        cases.extend((name, source, "", None) for name, source in SOURCE_BOUNDARIES.items())
        cases.extend((name, source, "", error) for name, (source, error) in ERRORS.items())
        for name, source, stdin, expected_error in cases:
            path = temporary / f"{name}.py"
            path.write_text(source)
            reference = run([args.python, str(path)], stdin)
            for level in args.opt_levels:
                binary = temporary / f"{name}-O{level}"
                command = [compiler, "build", str(path), "-o", str(binary), "--runtime", runtime, "--opt-level", level, "--clang", args.clang]
                command += build_flags
                if args.sanitize:
                    command.append("--sanitize")
                built = run(command)
                assert built.returncode == 0, (name, level, built.stderr)
                actual = run([str(binary)], stdin)
                assert actual.stdout == reference.stdout, (name, level, actual.stdout, reference.stdout, actual.stderr)
                if expected_error:
                    assert reference.returncode > 0 and expected_error in reference.stderr, (name, reference)
                    assert actual.returncode > 0, (name, actual)
                    if reference.stderr.splitlines()[-1] == expected_error:
                        assert actual.stderr == f"{expected_error}\n", (name, actual)
                    else:
                        assert actual.stderr.startswith(expected_error + ":"), (name, actual)
                else:
                    assert reference.returncode == actual.returncode == 0, (name, reference.stderr, actual.stderr)
                    assert not actual.stderr, (name, actual.stderr)
                assert "AddressSanitizer" not in actual.stderr, (name, actual.stderr)
                passed += 1
    print(f"Passed {passed} checks ({len(REJECTED)} rejected programs; 260 integer pairs; optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()
