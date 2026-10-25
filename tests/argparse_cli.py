"""Compare native argparse namespace values, help, usage, and unwinding with CPython."""

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(__file__).parent / "fixtures" / "argparse"
CASES = {
    "numbers": [
        [], ["--help"], ["-i", "１２_３"], ["-i", "123456789012345678901234567890"],
        ["-i", "0" * 4301], ["-i", "\x1c34\x1f"],
        ["-i", "bad", "-i", "1"], ["-i", "bad", "--help"], ["--help", "-i", "bad"],
        ["--scale=-0.0"], ["--scale", "1e-1000"], ["--scale=inf"], ["--scale=nan"],
        ["--scale=\U00011de0"],
        ["--scale", "1", "--scale", "bad"],
        ["--ids", "-02", "0_4"], ["--ids", "3", "bad"], ["--ids", "3", "2"],
        ["--choice=0.0"], ["--choice=2"], ["--maybe"], ["--maybe=01"], ["--maybe=42"],
        ["--unit", "ab"], ["--unit=c", "-.１２"], ["--label"], ["--label=-42"],
        ["--note", "red"], ["--note=42"],
    ],
    "counts": [
        [], ["a"], ["a", "b"], ["a", "b", "c", "d", "e"],
        ["--", "a", "b"], ["a", "--", "--", "b"], ["a", "b", "-v", "c"],
        ["a", "-v", "b", "c"], ["a", "b", "--", "-v"],
        ["--pair", "-v", "--flag", "a", "b"], ["--pair", "a", "--", "b"],
        ["--pair=x", "y"], ["--maybe", "--flag", "a", "b"], ["--maybe=", "a", "b"],
        ["--items", "--flag", "a", "b"], ["--items=x", "a", "b"], ["--help"],
    ],
    "groups": [[], ["--help"], ["foo", "check", "--help"], ["foo", "check"],
               ["--first", "a", "-v", "--alt=b", "foo", "check", "--long-setting", "c"]],
    "explicit_args": [[], ["first"], ["--", "-x"], ["first", "extra"], ["--help"]],
    "reader": [
        [],
        ["--help"],
        ["--it"],
        ["--in"],
        ["--items"],
        ["--items", "red", "green"],
        ["--items=red", "green"],
        ["--items", "-0.125", "-.９", "--verbose"],
        ["--items", "-\U00011de0"],
        ["--item-kind=blue", "--items", "red"],
        ["--items", "red", "--", "tail"],
        ["--help=none"],
        ["-=", "--help"],
        ["-=value"],
        ["-hhh=x"],
        ["-hh-x"],
        ["-hx"],
        ["--label", "--items", "a"],
        ["--unknown", "red", "--help"],
        ["--items", "one", "--items=two", "--verbose", "--verbose"],
        [
            "--items",
            "red",
            "--output",
            "dir/file",
            "--label",
            "☃",
            "--input-path",
            ".//x/",
            "--item-kind",
            "long-blue",
        ],
        ["--verbose=x", "--items", "red"],
        ["--items", "a", "--debug", "--locations", ".", "foo//bar"],
        ["--items", "a", "--locations=/foo/./bar"],
    ],
    "commands": [
        [],
        ["--help"],
        ["unknown"],
        ["--", "read"],
        ["--", "--help"],
        ["read", "--help"],
        ["write", "--help"],
        ["--label", "sample", "read"],
        ["read", "--first", "dir/a", "--second", "dir/b"],
        ["write"],
        ["write", "--text", "okay"],
        ["write", "--text"],
        ["read", "--text", "a"],
        ["--other", "write"],
        ["--other", "write", "--text", "ok", "extra"],
    ],
    "positionals": [
        [], ["check"], ["check", "base"], ["check", "base", "compare"],
        ["format", "base", "compare", "-v"], ["-vv", "format", "/a", "./b"],
        ["check", "--verbose", "base", "compare", "-cjson"],
        ["check", "-vcjson", "base", "compare"],
        ["check", "-c", "text", "base", "compare"],
        ["check", "--mode=text", "base", "compare"],
        ["check", "--", "-base", "--compare"],
        ["check", "base", "compare", "extra"], ["other", "base", "compare"],
        ["check", "base", "compare", "--mode", "missing"],
        ["-vh"], ["-hv"], ["-v=1"], ["-hv=xx"], ["-vh=xx"], ["-hc"], ["-hc", "other"],
        ["check", "base", "compare", "--"], ["check", "base", "compare", "-v", "--"],
        ["check", "base", "compare", "extra", "--"], ["-vv=1"], ["-vx", "check", "b", "c"],
        ["--mo", "json", "check", "base", "compare"],
    ],
    "many_positionals": [
        [], ["--"], ["one"], ["./one", "two/./three"], ["-v", "one", "two"],
        ["one", "-v", "two"], ["one", "--", "-v", "--"], ["--", "--", "tail"],
        ["one", "--unknown", "two"], ["--unknown", "one"], ["one", "--help"],
    ],
    "method_args": [[]],
    "numeric_defaults": [[], ["--help"], ["word"], ["--amount", "2"], ["--amount=2", "word"],
                         ["--amount", "bad", "--help"], ["--help", "--amount", "bad"],
                         ["range", "--big", "4999", "100000000000000000001"],
                         ["radix", str(2**80)], ["radix", str(2**80 + 3)], ["radix", "--help"]],
    "two": [[], ["--help"], ["--mo=lar"], ["--flag"], ["--paths", "one", "two"]],
}
REJECTED = {
    "computed_flag": 'parser.add_argument("--" + "item")',
    "repeated_flag": 'parser.add_argument("-v", "--verbose")\n    parser.add_argument("-v")',
    "invalid_arity": 'parser.add_argument("input", nargs="bad")',
    "zero_arity": 'parser.add_argument("input", nargs=0)',
    "mixed_positionals": 'parser.add_argument("input", "--other")',
    "foreign_converter": 'parser.add_argument("--number", type=complex)',
    "choice_expression": 'parser.add_argument("--key", choices=list("ab"))',
    "duplicate_destination": 'parser.add_argument("--a-b")\n    parser.add_argument("--a_b")',
    "escaped_builder": "copy = parser",
    "bad_template": 'parser.add_argument("--key", help="%(value)s")',
}


def result(command, env):
    completed = subprocess.run(
        command, env=env, capture_output=True, timeout=30, check=False
    )
    return completed.returncode, completed.stdout, completed.stderr


def main():
    options = argparse.ArgumentParser(description=__doc__)
    options.add_argument("--compiler", required=True, type=Path)
    options.add_argument("--runtime", required=True, type=Path)
    options.add_argument("--python", default=sys.executable)
    options.add_argument("--clang", default="clang")
    options.add_argument("--target")
    options.add_argument("--lto-linker", type=Path)
    options.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    options.add_argument("--sanitize", action="store_true")
    args = options.parse_args()
    compile_flags = [
        "--quiet",
        "--clang",
        args.clang,
        "--runtime",
        args.runtime.resolve(),
    ]
    if args.target:
        compile_flags += ["--target", args.target]
    if args.lto_linker:
        compile_flags += ["--lto-linker", args.lto_linker.absolute()]
    if args.sanitize:
        compile_flags += ["--sanitize"]
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-argparse-") as directory:
        for name, cases in CASES.items():
            source = FIXTURES / (name + ".py")
            native = Path(directory) / name
            samples = [(arguments, 80, False) for arguments in cases]
            for width in (16, 34, 55, 111):
                for arguments in (["--help"], ["--not-an-option"]):
                    samples.extend((arguments, width, color) for color in (False, True))
                if name == "groups":
                    samples.extend((["foo", "check", "--help"], width, color) for color in (False, True))
            for width in (" 34\t", "+3_4", "٣٤", "3__4", "9" * 20, "0" * 4300 + "34"):
                samples.append((["--help"], width, False))
            for level in args.opt_levels:
                subprocess.run(
                    [
                        args.compiler,
                        "build",
                        source,
                        "-o",
                        native,
                        "--opt-level",
                        level,
                        *compile_flags,
                    ],
                    check=True,
                )
                for arguments, width, color in samples:
                    env = {
                        **os.environ,
                        "COLUMNS": str(width),
                        "PYTHON_COLORS": "1" if color else "0",
                        "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1",
                    }
                    expected = result([args.python, source, *arguments], env)
                    actual = result([native, *arguments], env)
                    if expected != actual:
                        raise AssertionError(
                            (name, arguments, width, color, expected, actual)
                        )
                    checks += 1
        # An integer written in the source is already a value. A large hex
        # default/const remains usable even when decimal argv conversion is limited.
        source = Path(directory) / "literal_default.py"
        literal = "0x" + "f" * 560
        source.write_text(
            "import argparse\ndef read():\n    parser = argparse.ArgumentParser()\n"
            f"    parser.add_argument('-n', nargs='?', type=int, default={literal}, const={literal})\n"
            f"    return parser.parse_args()\nprint(read().n == {literal})\n"
        )
        native = source.with_suffix("")
        env = {**os.environ, "PYTHONINTMAXSTRDIGITS": "640", "PYTHON_COLORS": "0"}
        for level in args.opt_levels:
            subprocess.run([args.compiler, "build", source, "-o", native, "--opt-level", level, *compile_flags], check=True)
            for arguments in ([], ["-n"], ["-n", "1" + "0" * 650]):
                expected = result([args.python, source, *arguments], env)
                actual = result([native, *arguments], env)
                if actual != expected:
                    raise AssertionError(("literal_default", arguments, level, expected, actual))
                checks += 1
        for name, declaration in REJECTED.items():
            source = Path(directory) / (name + ".py")
            source.write_text(
                "from argparse import ArgumentParser\ndef read():\n    parser = ArgumentParser()\n"
                f"    {declaration}\n    return parser.parse_args()\nread()\n"
            )
            check = subprocess.run(
                [args.compiler, "check", source], capture_output=True, check=False
            )
            if (
                check.returncode == 0
                or b"argparse" not in check.stderr.lower()
                and b"add_argument" not in check.stderr
            ):
                raise AssertionError((name, check.returncode, check.stderr))
        # The successful guard relies on a stable receiver and a read-only field.
        source = Path(directory) / "bad_field.py"
        prelude = "from argparse import ArgumentParser\nfrom pathlib import Path\ndef read():\n    p = ArgumentParser()\n    p.add_argument('--out', type=Path)\n    return p.parse_args()\na = read()\n"
        for statement in [
            "a.out = Path('else')",
            "print(a.out.name)",
            "if a.out:\n    a = read()\n    print(a.out.name)",
        ]:
            source.write_text(prelude + statement + "\n")
            check = subprocess.run(
                [args.compiler, "check", source], capture_output=True, check=False
            )
            if check.returncode == 0:
                raise AssertionError(statement)
        # A field unique to one child is accessible only after dispatch.
        source = Path(directory) / "unchecked_command.py"
        commands = (FIXTURES / "commands.py").read_text()
        for wrong in ["print(args.text)", "print(args.first)"]:
            source.write_text(commands.replace("    print(args.label)", "    " + wrong))
            checked = subprocess.run([args.compiler, "check", source], capture_output=True)
            if checked.returncode == 0:
                raise AssertionError(wrong)
        source.write_text(commands.replace("    return args\n", "    print(args.text)\n    return args\n"))
        checked = subprocess.run([args.compiler, "check", source], capture_output=True)
        if checked.returncode == 0 or b"unknown field" not in checked.stderr:
            raise AssertionError(checked.stderr)
        source.write_text(
            "import argparse\ndef count(args: argparse.Namespace) -> int:\n    return 1\n"
            "print(count(0))  # ty: ignore[invalid-argument-type]\n"
        )
        checked = subprocess.run([args.compiler, "check", source], capture_output=True)
        if checked.returncode == 0 or b"argparse.Namespace requires" not in checked.stderr:
            raise AssertionError(checked.stderr)
    print(
        f"Validated {checks} argparse process comparisons and {len(REJECTED) + 7} compile-time rejections"
    )


if __name__ == "__main__":
    main()
