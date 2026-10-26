"""Replay CPython's unchanged mixed str.maketrans/translate test with native dictionary lookups."""
import argparse
import ast
import hashlib
import inspect
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
from test.test_str import StrTest


def original(dispatch, direct=None):
    class Input(str):
        def translate(self, *args):
            return dispatch(str(self), args)

    class Case(StrTest):
        def assertEqual(self, actual, expected, *args, **kwargs):
            # A str literal dispatches in CPython. These direct assertions
            # consume an independently compiled lookup of the same table.
            if direct is not None and sys._getframe(1).f_code is StrTest.test_maketrans_translate.__code__:
                actual = next(direct)
            return super().assertEqual(actual, expected, *args, **kwargs)

        def checkequalnofix(self, expected, source, name, *args):
            return super().checkequalnofix(expected, Input(source), name, *args)

        def assertRaises(self, expected, function, *args, **kwargs):
            if getattr(function, "__name__", "") == "translate":
                return super().assertRaises(expected, dispatch, function.__self__, args)
            return super().assertRaises(expected, function, *args, **kwargs)

    return Case("test_maketrans_translate").run()


def literal_lookups(method):
    # The twelve direct assertions use a literal receiver and str.maketrans on
    # literal arguments. Keep maketrans on CPython and feed its real table to Imre.
    lookups = []
    for line in ast.parse(textwrap.dedent(method)).body[0].body:
        if not isinstance(line, ast.Expr) or not isinstance(line.value, ast.Call):
            continue
        call = line.value
        if not isinstance(call.func, ast.Attribute) or call.func.attr != "assertEqual":
            continue
        expr = call.args[0]
        assert expr.func.attr == "translate"
        make, = expr.args
        assert make.func.value.id == "str" and make.func.attr == "maketrans" and not make.keywords
        lookups.append((ast.literal_eval(expr.func.value), (str.maketrans(*map(ast.literal_eval, make.args)),)))
    return lookups


def bounded_expansion(args, binary, mode, level):
    if sys.platform != "linux":
        return
    import resource
    fixture = Path(__file__).resolve().parent / "fixtures/text_limits/translate.py"
    def limited():
        resource.setrlimit(resource.RLIMIT_AS, (128 << 20, 128 << 20))
    def run(command):
        return subprocess.run(command, capture_output=True, preexec_fn=limited, check=True, timeout=30)
    oracle = run([sys.executable, "-I", "-B", fixture])
    assert oracle.stdout == b"MemoryError()\n''\n32768\n'ok'\n" and not oracle.stderr
    subprocess.run([args.compiler, "build", fixture, "--no-cache", "--runtime", args.runtime,
                    "--clang", args.clang, "--opt-level", level, "-o", binary, *mode], check=True)
    actual = run([binary])
    assert (actual.stdout, actual.stderr) == (oracle.stdout, oracle.stderr)


def annotation(table):
    keys = {type(k) for k in table}
    values = {type(v) for v in table.values()}
    if keys not in ({int}, {str}):
        return None
    scalar = values - {type(None)}
    if len(scalar) > 1 or not scalar <= {str, int}:
        return None
    value = next(iter(scalar)).__name__ if scalar else "None"
    if scalar and type(None) in values:
        value += " | None"
    return f"dict[{next(iter(keys)).__name__}, {value}]"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    method = inspect.getsource(StrTest.test_maketrans_translate)
    assert hashlib.sha256(method.encode()).hexdigest() == "63f7cab45f2cbde2b9da34862e5f21d71d05acdb33fc620643394439fe26be9a"
    queries = []
    def record(text, params):
        queries.append((text, params))
        return text.translate(*params)
    assert original(record).wasSuccessful()
    indirect = len(queries)
    queries.extend(literal_lookups(method))
    lines, native = [], []
    for index, (text, params) in enumerate(queries):
        if len(params) == 1 and isinstance(params[0], dict):
            descriptor = annotation(params[0])
            if descriptor:
                var = f"table_{index}"
                lines.append(f"{var}: {descriptor} = {params[0]!r}")
                values = var
            else:
                # The original inline table mixes str, int and None. No variable
                # annotation is needed; the operation determines its exact layout.
                assert set(map(type, params[0].values())) == {str, int, type(None)}
                values = repr(params[0])
        else:
            values = ", ".join(map(repr, params))
        native.append(index)
        lines.extend((f"try: print('S' + repr({text!r}.translate({values})))",
                      "except TypeError as e: print('T' + str(e))",
                      "except ValueError as e: print('V' + str(e))"))
    assert indirect == 14 and len(queries) == 26 and len(native) == 26
    def replay(answers):
        current = iter(enumerate(queries[:indirect]))
        def lookup(text, params):
            index, expected = next(current)
            assert (text, params) == expected
            value = answers[index]
            if value[0] == "T": raise TypeError(value[1:])
            if value[0] == "V": raise ValueError(value[1:])
            assert value[0] == "S"
            return ast.literal_eval(value[1:])
        direct = (ast.literal_eval(answers[i][1:]) for i in range(indirect, len(queries)))
        return original(lookup, direct)
    with tempfile.TemporaryDirectory() as tmp:
        source, binary = Path(tmp)/"translations.py", Path(tmp)/"translations"
        source.write_text("\n".join(lines) + "\n")
        for mode in ([], ["--reachable-only"]):
            for level in ("0", "3"):
                subprocess.run([args.compiler, "build", source, "--no-cache", "--runtime", args.runtime,
                                "--clang", args.clang, "--opt-level", level, "-o", binary, *mode], check=True)
                run = subprocess.run([binary], capture_output=True, check=True, text=True)
                assert not run.stderr
                results = dict(zip(native, run.stdout.splitlines(), strict=True))
                result = replay(results)
                assert result.wasSuccessful(), (result.errors, result.failures)
                assert replay({**results, native[0]: "S'wrong native result'"}).failures
                assert replay({**results, indirect: "S'wrong native literal lookup'"}).failures
                print(f"{'reachable' if mode else 'whole'} O{level}: 14 native helper/error calls "
                      "+ 12 native results supplied to unchanged direct assertions; "
                      "all maketrans construction and errors remain on the host")
                bounded_expansion(args, binary, mode, level)


if __name__ == "__main__":
    main()
