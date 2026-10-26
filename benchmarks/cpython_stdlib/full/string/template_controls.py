"""Differential checks independent of the CPython string package's 54 test IDs."""
import ast
import hashlib
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
LOGGING_BLOB = "9005f1ef865c9096e6035feb7a53a98f8dade705"
METHOD_SHA256 = "af031547b3c9559800e0452ffbae9c2b89fa3a25298890e3dee1bf438c909bfc"
STYLE_FIXTURE = b"""from string import Template

class Style:
    _fmt: str

    def __init__(self, fmt: str) -> None:
        self._fmt = fmt

"""
CHECK = b"""
for fmt in ["$message", "${asctime}: $message", "$$ $x $$", "prefix $", "none", "$$", "${bad", "$message $", "${bad-name}"]:
    try:
        Style(fmt).validate()
        print("valid")
    except ValueError as exc:
        print(str(exc))
"""
# Keep arbitrary callback/mutation effects outside the short truthy-read proof.
DIAGNOSTICS = {
    "empty_match": ("import re\np = re.compile('a*')\nfor m in p.finditer(''): print(m.group(0))", "cannot match an empty string"),
    "custom_pattern": ("from string import Template\nclass T(Template):\n    pattern = 'x'\nprint(T('x').is_valid())", "overriding a fixed class binding"),
    "changed_value": ("def f(d: dict[str, str | None], s: set[str]) -> None:\n    if d['x']:\n        d['x'] = None\n        s.add(d['x'])", "expected str, got str | None"),
    "changed_key": ("def f(d: dict[str, str | None], s: set[str]) -> None:\n    if d['x']:\n        s.add(d['y'])", "expected str, got str | None"),
}


def oracle(args, program, work):
    return subprocess.run([args.python, "-I", "-B", "-"], input=program, capture_output=True,
                          check=True, cwd=work).stdout


def controls(args, level, work, compile_run):
    method = (HERE / "logging_validate.method").read_bytes()
    assert hashlib.sha256(method).hexdigest() == METHOD_SHA256
    if args.cpython_source:
        # Verify the exact enclosing file and byte span at the pinned commit.
        source = (args.cpython_source / "Lib/logging/__init__.py").read_bytes()
        assert hashlib.sha1(b"blob " + str(len(source)).encode() + b"\0" + source).hexdigest() == LOGGING_BLOB
        cls = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == "StringTemplateStyle")
        original = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "validate")
        assert b"".join(source.splitlines(keepends=True)[original.lineno-1:original.end_lineno]) == method
    observations = 0
    for name in ("template_differential.py", "template_object_values.py", "template_exception_values.py"):
        source = (HERE / name).read_bytes()
        expected = oracle(args, source, work)
        assert compile_run(args, level, source, work) == expected, f"{name} differs from CPython"
        observations += len(expected.splitlines())
    logging = oracle(args, b"from logging import StringTemplateStyle as Style\n" + CHECK, work)
    assert compile_run(args, level, STYLE_FIXTURE + method + CHECK, work) == logging, "upstream logging validator differs"
    for name, (source, expected_error) in DIAGNOSTICS.items():
        path = work / f"{name}.py"
        path.write_text(source + "\n")
        r = subprocess.run([args.imre.resolve(), "check", path], capture_output=True, text=True, cwd=work)
        assert r.returncode and expected_error in r.stderr, (name, r.returncode, r.stderr)
    return observations, len(logging.splitlines())
