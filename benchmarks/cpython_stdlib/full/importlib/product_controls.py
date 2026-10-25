"""Check exact direct semantics and explicit unsupported operations outside the upstream ledger."""

import subprocess
import sys
from importlib.util import cache_from_source
from pathlib import Path

from suite_native import NativeCompileError


def check(lang, root):
    # This program keeps and calls actual native loader objects. No transport
    # or CPython fixture supplies the results of its product operations.
    original = Path(__file__).with_name("controls.py")
    directory = root / "direct"
    directory.mkdir()
    exe = lang.build(original.read_bytes(), directory)
    expected = subprocess.run(
        [sys.executable, "-I", "-S", original], check=True, capture_output=True
    )
    actual = subprocess.run([exe], check=True, capture_output=True)
    if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
        raise AssertionError((actual, expected))

    prefix = "from importlib.machinery import ModuleSpec\na = ModuleSpec('x', None)\nb = ModuleSpec('x', None)\n"
    boundaries = [
        (
            "import importlib\nprint(importlib.machinery.ModuleSpec('x', None).name)",
            "AttributeError",
            "import importlib.machinery before",
        ),
        (prefix + "print(a == b)", "True", "ModuleSpec rich comparison"),
        (prefix + "print((a,) == (b,))", "True", "ModuleSpec rich comparison"),
        (prefix + "print(a[0])", "TypeError", "indexing requires"),
        (
            prefix + "a.parent = 'new'",
            "AttributeError",
            "ModuleSpec.parent is read-only",
        ),
        (
            prefix + "a.loader = ['changed']\nprint(a.loader)",
            "['changed']",
            "expected None, got list[str]",
        ),
    ]
    for i, (source, python, diagnostic) in enumerate(boundaries):
        where = root / f"boundary-{i}"
        where.mkdir()
        ref = subprocess.run(
            [sys.executable, "-I", "-S", "-c", source],
            check=False,
            capture_output=True,
            text=True,
        )
        if python.endswith("Error"):
            assert ref.returncode != 0 and ref.stderr.strip().splitlines()[
                -1
            ].startswith(python + ":"), ref
        else:
            assert (ref.returncode, ref.stdout.strip(), ref.stderr) == (
                0,
                python,
                "",
            ), ref
        try:
            lang.build(source.encode(), where)
        except NativeCompileError as err:
            if diagnostic not in str(err):
                raise
        else:
            raise AssertionError(f"lost explicit unsupported operation: {source}")

    # Source bytecode requires an interpreter cache tag, not a guessed filename.
    source = (
        prefix
        + """a.origin = 'dir/mod.py'
a.has_location = True
try:
    path = a.cached
    if path is None:
        raise AssertionError('unexpected absent path')
    print(path)
except NotImplementedError as exc:
    print(str(exc))
"""
    )
    directory = root / "source-cache"
    directory.mkdir()
    ref = subprocess.run(
        [sys.executable, "-I", "-S", "-c", source],
        check=True,
        capture_output=True,
        text=True,
    )
    assert ref.stdout.strip() == cache_from_source("dir/mod.py")
    binary = lang.build(source.encode(), directory)
    actual = subprocess.run([binary], check=True, capture_output=True, text=True)
    assert (actual.stdout, actual.stderr) == (
        "source cache paths require a bytecode cache tag\n",
        "",
    )
