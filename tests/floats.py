"""Compare binary64 arithmetic, exact integer boundaries, and typed storage with CPython."""

import argparse
import math
import random
import shutil
import struct
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import require_result, run
from expressions import reference

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "floats"

REJECTED = {
    "annotation_does_not_convert": "value: float = 1\n",
    "return_does_not_convert": "def bad() -> float:\n    return 1\n",
    "argument_does_not_convert": "def bad(value: float) -> None:\n    pass\nbad(1)\n",
    "default_does_not_convert": "def bad(value: float = 1) -> None:\n    pass\n",
    "optional_does_not_convert": "value: float | None = 1\n",
    "list_does_not_convert": "values: list[float] = [1.0, 2]\n",
    "dict_does_not_convert": 'values: dict[str, float] = {"one": 1}\n',
    "tuple_does_not_convert": "values: tuple[float, float] = (1.0, 2)\n",
    "conditional_does_not_convert": "value = 1.0 if True else 1\n",
    "alias_write_does_not_convert": "values = [1.0]\nalias = values\nalias[0] = 1\n",
    "field_does_not_convert": "class Item:\n    value: float\n    def __init__(self) -> None:\n        self.value = 1\n",
    "float_dictionary_key": "values = {1.0: 2}\n",
    "float_set_element": "values = {1.0}\n",
    "float_set_comprehension": "values = {float(index) for index in range(2)}\n",
    "float_exponentiation": "print(2.0 ** 2)\n",
    "complex_literal": "print(1j)\n",
    "float_boolean_arithmetic": "print(1.0 + True)\n",
    "float_boolean_comparison": "print(1.0 == True)\n",
    "float_index": "print([1][0.0])\n",
    "float_slice": "print([1][:1.0])\n",
    "integer_true_division_storage": "value: int = 1 / 2\n",
    "integer_inplace_true_division": "value = 1\nvalue /= 2\n",
    "float_none": "print(float(None))\n",
    "float_keyword": "print(float(value=1))\n",
    "unsupported_exponential_format": 'print(f"{1.0:.2e}")\n',
    "optional_float_unchecked": "def bad(value: float | None) -> float:\n    return value + 1.0\n",
}


def literal(value: float) -> str:
    return repr(value) if math.isfinite(value) else f"float({str(value)!r})"


def generated() -> dict[str, str]:
    rng = random.Random(0x0A17F10A7)
    floats = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        -0.1,
        0.5,
        -0.5,
        1.5,
        -1.5,
        1e-5,
        1e-4,
        1e15,
        1e16,
        9007199254740992.0,
        9007199254740994.0,
        5e-324,
        -5e-324,
        2.225073858507201e-308,
        2.2250738585072014e-308,
        1e308,
        -1e308,
        sys.float_info.max,
        -sys.float_info.max,
        float("inf"),
        float("-inf"),
        float("nan"),
    ]
    matrix = "values: list[float] = [" + ", ".join(map(literal, floats)) + "]\n"
    matrix += "for left in values:\n    for right in values:\n"
    for op in ["+", "-", "*"]:
        matrix += f"        print(left {op} right)\n"
    for op in ["/", "//", "%"]:
        matrix += f"        try:\n            print(left {op} right)\n        except ZeroDivisionError:\n            print('zero')\n"
    for op in ["==", "!=", "<", "<=", ">", ">="]:
        matrix += f"        print(left {op} right)\n"

    integers = [
        0,
        1,
        -1,
        2**53 - 1,
        2**53,
        2**53 + 1,
        -(2**53 + 1),
        2**62 - 1,
        2**62,
        10**400,
        -(10**400),
    ]
    mixed = "integers = [" + ", ".join(map(str, integers)) + "]\n"
    mixed += "floats: list[float] = [" + ", ".join(map(literal, floats)) + "]\n"
    mixed += "for integer in integers:\n    for floating in floats:\n"
    for op in ["==", "!=", "<", "<=", ">", ">="]:
        mixed += f"        print(integer {op} floating)\n        print(floating {op} integer)\n"
    for op in ["+", "-", "*", "/", "//", "%"]:
        for left, right in [("integer", "floating"), ("floating", "integer")]:
            mixed += f"        try:\n            print({left} {op} {right})\n        except OverflowError:\n            print('overflow')\n        except ZeroDivisionError:\n            print('zero')\n"

    random_floats: list[float] = []
    while len(random_floats) < 512:
        value = struct.unpack("!d", rng.getrandbits(64).to_bytes(8))[0]
        if math.isfinite(value):
            random_floats.append(value)
    formatting = (
        "values: list[float] = ["
        + ", ".join(map(literal, floats + [900719925474099.25] + random_floats))
        + "]\n"
    )
    formatting += "for value in values:\n    print(value)\n    print(str(value))\n    print(f'{value!s}:{value:}')\n    print(+value)\n    print(-value)\n    print(float(value))\n    print(float(str(value)))\n    try:\n        print(int(value))\n    except ValueError:\n        print('nan')\n    except OverflowError:\n        print('infinite')\n"
    formatting += "print(float())\nprint(float(True))\nprint(float(False))\n"

    max_integer = int(sys.float_info.max)
    pairs = [(10**400, 10**400), (10**400 + 1, 10**400 - 1), (0, -1), (1, 0)]
    for exponent in [-1076, -1075, -1074, -1023, -1022, -53, 0, 53, 1023, 1024]:
        for delta in [-1, 0, 1]:
            numerator = (2**1200 + delta) << max(exponent, 0)
            denominator = 2**1200 << max(-exponent, 0)
            pairs.extend(
                [
                    (numerator, denominator),
                    (-numerator, denominator),
                    (numerator, -denominator),
                ]
            )
    pairs.extend(
        (max_integer + delta, 1) for delta in [0, 2**970 - 1, 2**970, 2**970 + 1]
    )
    for delta in [-1, 0, 1]:
        for _ in range(16):
            pairs.append((2**53 + delta, rng.randrange(-(2**53), 2**53)))
    for _ in range(256):
        size = rng.randrange(100, 6000)
        numerator = rng.getrandbits(size) | 1
        denominator = rng.getrandbits(size) | 1
        exponent = rng.randrange(-1080, 1030)
        numerator <<= max(exponent, 0)
        denominator <<= max(-exponent, 0)
        pairs.append((numerator, denominator))
    ratios = (
        "pairs: list[tuple[int, int]] = [\n"
        + "\n".join(f"    ({left}, {right})," for left, right in pairs)
        + "\n]\n"
    )
    ratios += "for left, right in pairs:\n    try:\n        print(left / right)\n    except ZeroDivisionError:\n        print('zero')\n    except OverflowError:\n        print('overflow')\n"
    conversions = (
        "values = ["
        + ", ".join(
            map(
                str,
                integers
                + [max_integer, max_integer + 2**970 - 1, max_integer + 2**970],
            )
        )
        + "]\n"
    )
    conversions += "for value in values:\n    try:\n        print(float(value))\n    except OverflowError:\n        print('overflow')\n"

    text = [
        "0",
        "-0",
        "+.5",
        "1.",
        "1_0.5_0e-0_1",
        "-1e-9999",
        "1e9999",
        "INF",
        "-Infinity",
        "+NaN",
        "-nan",
        " \u2003-١_٢.٥e+٢\u00a0",
        "１２.３",
        "𝟏𝟐.𝟓",
        "",
        "_1",
        "1_",
        "1__2",
        "1_.0",
        "1._0",
        "1e_2",
        "1e+_2",
        ".",
        "1e",
        "0x1",
        "nan(1)",
        "1\0",
        "1 2",
        "²",
        "\x1c1",
    ]
    parsing = (
        "texts = "
        + repr(text)
        + "\nfor text in texts:\n    try:\n        print(float(text))\n    except ValueError:\n        print('invalid')\n"
    )
    return {
        "arithmetic_matrix": matrix,
        "mixed_matrix": mixed,
        "formatting": formatting,
        "ratios": ratios,
        "conversions": conversions,
        "parsing": parsing,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument(
        "--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"]
    )
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    flags = ["--target", args.target] if args.target else []
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags.append("--sanitize")
    checks = 0
    with tempfile.TemporaryDirectory(prefix="relar-floats-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(FIXTURES, project)
        for name, source in generated().items():
            (project / f"{name}.py").write_text(source)
        for source in sorted(project.glob("*.py")):
            expected = reference(args.python, source, temporary, None)
            for level in args.opt_levels:
                binary = temporary / f"{source.stem}-O{level}"
                built = run(
                    [
                        compiler,
                        "build",
                        str(source),
                        "-o",
                        str(binary),
                        "--runtime",
                        runtime,
                        "--clang",
                        args.clang,
                        "--opt-level",
                        level,
                        *flags,
                    ],
                    temporary,
                    # Wide-integer fixtures need more ThinLTO time on CI workers.
                    timeout=300 if args.lto_linker else 60,
                )
                assert built.returncode == 0 and not built.stderr, (
                    source.name,
                    level,
                    built.stdout,
                    built.stderr,
                )
                actual = run([str(binary)], temporary)
                require_result(actual, None)
                if actual.stdout != expected.stdout:
                    actual_lines, expected_lines = (
                        actual.stdout.splitlines(),
                        expected.stdout.splitlines(),
                    )
                    index = next(
                        (
                            i
                            for i, pair in enumerate(zip(actual_lines, expected_lines))
                            if pair[0] != pair[1]
                        ),
                        min(len(actual_lines), len(expected_lines)),
                    )
                    raise AssertionError(
                        (
                            source.name,
                            level,
                            index + 1,
                            actual_lines[max(0, index - 2) : index + 3],
                            expected_lines[max(0, index - 2) : index + 3],
                        )
                    )
                checks += 1
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(source)
            result = run([compiler, "check", str(path)], temporary)
            assert (
                result.returncode > 0
                and result.stderr
                and "panicked" not in result.stderr
            ), (name, result)
            checks += 1
    print(
        f"Passed {checks} float checks ({len(REJECTED)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
