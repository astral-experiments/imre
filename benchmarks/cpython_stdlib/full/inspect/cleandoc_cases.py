"""Run the original inspect subtests against a native string operation."""
import ast
import hashlib
import inspect
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

from suite_evidence import roles
from suite_native import method_bytes

HERE = Path(__file__).resolve().parent
OWNER = "TestRetrievingSourceCode"
METHOD = "test_cleandoc"
CASE = f"test.test_inspect.test_inspect.{OWNER}.{METHOD}"

PUBLIC_CALLS = b"""import inspect
from inspect import cleandoc as clean

def argument(value: str) -> str:
    print("doc argument")
    if not value:
        raise ValueError("before cleandoc")
    return value

def guarded(doc: str | None) -> str:
    if doc is None:
        return "no doc"
    return inspect.cleandoc(doc=doc)

saved = inspect.cleandoc
print(clean(doc=argument(" first\\n    second")))
print(saved("   alias"))
print(guarded(None))
print(guarded("\\n    narrowed\\n"))
try:
    clean(doc=argument(""))
except ValueError as exc:
    print(str(exc))
"""

INPUTS = (
    "", " ", "\t", "\n\n", " single  ", "\n \n  \n   ", "\t\n\t",
    "  head\n   \n  body  \n", "head\n    \n", "\r\n  one\r\n    two\r\n",
    "head\r reused\r  paragraphs", "\n    é\tX\n\t  雪\n", "a\t\tb\r\tc",
    "\x00\n    before\x00after\n  \x00", "\t\u00a0\n  \u2003π\n  雪",
    "\u2028alpha\u2029beta\u0085gamma\vdelta\fform", " 🙂\tX\n   \u0301tab\tend ",
)

# Each static diagnostic is checked against the actual CPython result for the same source.
BOUNDARIES = (
    ("cleandoc()", "missing required argument", "TypeError: cleandoc() missing 1 required positional argument: 'doc'"),
    ('cleandoc("a", "b")', "too many positional arguments", "TypeError: cleandoc() takes 1 positional argument but 2 were given"),
    ('cleandoc(text="a")', 'unexpected keyword argument "text"', "TypeError: cleandoc() got an unexpected keyword argument 'text'"),
    ('cleandoc("a", doc="b")', 'multiple values for argument "doc"', "TypeError: cleandoc() got multiple values for argument 'doc'"),
    ("cleandoc(42)", "expected str, got int", "AttributeError: 'int' object has no attribute 'expandtabs'"),
    ("cleandoc(None)", "expected str, got None", "AttributeError: 'NoneType' object has no attribute 'expandtabs'"),
    ('cleandoc(b"x")', "expected str, got bytes", "TypeError: a bytes-like object is required, not 'str'"),
    ('def clean(doc: object) -> str:\n    return cleandoc(doc)\nprint(clean(" ok"))',
     "expected str, got object value", "ok"),
    ('def clean(doc: str | None) -> str:\n    return cleandoc(doc)\nprint(clean(" ok"))',
     "expected str, got str | None", "ok"),
    ('class Duck:\n    def expandtabs(self) -> str:\n        return " duck"\nprint(cleandoc(Duck()))',
     "expected str, got", "duck"),
    ('fn = cleandoc\nprint(fn(doc=" ok"))', "indirect calls require the full positional", "ok"),
    ("import __imre_inspect_functions__", "the private inspect implementation is not an importable module",
     "ModuleNotFoundError: No module named '__imre_inspect_functions__'"),
)


def native_clean(binary, input_file, text):
    input_file.write_bytes(text.encode("utf-8"))
    result = subprocess.run([binary, input_file], capture_output=True, check=True, timeout=15)
    if result.stderr:
        raise AssertionError(result.stderr)
    return result.stdout.decode("utf-8")


def fixture(tool, directory, source):
    public = tool.build(PUBLIC_CALLS, directory)
    expected = subprocess.run([sys.executable, "-EB", directory / "case.py"], check=True, capture_output=True)
    actual = subprocess.run([public], check=True, capture_output=True, timeout=15)
    assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr)
    binary = tool.build((HERE / "cleandoc_worker.py").read_bytes(), directory)
    input_file = directory / "input.utf8"
    for text in INPUTS:
        assert native_clean(binary, input_file, text) == inspect.cleandoc(text), repr(text)

    for snippet, diagnostic, oracle in BOUNDARIES:
        expression = "from inspect import cleandoc\n" + snippet + "\n"
        host = subprocess.run([sys.executable, "-EB", "-c", expression], capture_output=True, text=True)
        if host.returncode:
            assert host.stderr.splitlines()[-1] == oracle, (snippet, host)
        else:
            assert host.stdout == oracle + "\n" and not host.stderr, (snippet, host)
        entry = directory / "boundary.py"
        entry.write_text(expression)
        opts = ["--reachable-only"] if tool.reachable_only else []
        native = subprocess.run([tool.imre.resolve(), "check", entry, *opts], capture_output=True, text=True, timeout=30)
        assert native.returncode == 1 and diagnostic in native.stderr, (snippet, native)

    original_source = source.read_bytes()
    owner, = (node for node in ast.parse(original_source).body if isinstance(node, ast.ClassDef) and node.name == OWNER)
    table, = (node for node in owner.body if isinstance(node, ast.Assign)
              and any(isinstance(target, ast.Name) and target.id == "cleandoc_testdata" for target in node.targets))
    table_bytes = b"".join(original_source.splitlines(keepends=True)[table.lineno - 1:table.end_lineno])
    rows = ast.literal_eval(table.value)

    def execute(case, original):
        assert case.cleandoc_testdata == rows
        seen = []

        def operation(text):
            seen.append(text)
            return native_clean(binary, input_file, text)

        with patch.object(inspect, "cleandoc", operation):
            original()
        assert seen == [text for text, _ in rows], seen

        # Both controls call the native implementation, corrupting only its returned string.
        for corrupt, expected_failures in ((lambda s: s.partition("\0")[0], {2}),
                                           (lambda s: s.rstrip(" "), {1, 5})):
            result = unittest.TestResult()
            with patch.object(inspect, "cleandoc", lambda text: corrupt(operation(text))):
                type(case)(METHOD).run(result)
            assert not result.errors and {sub.params["i"] for sub, _ in result.failures} == expected_failures

        return {
            "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
            "evidence": {
                "method_sha256": hashlib.sha256(method_bytes(source, OWNER, METHOD)).hexdigest(),
                "table_sha256": hashlib.sha256(table_bytes).hexdigest(),
                "operation": "All six original subtests call the compiled public inspect.cleandoc.",
                "fixture": "CPython runs GetSourceBase.setUp, the original table, subTest contexts, and assertions.",
                "negative_controls": "Truncating at NUL fails subtest 2. Removing trailing spaces fails subtests 1 and 5.",
            },
        }

    return execute
