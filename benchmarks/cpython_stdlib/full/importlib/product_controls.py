"""Check exact direct semantics and explicit unsupported operations outside the upstream ledger."""

import subprocess
import sys
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
        (
            "import importlib\nprint(importlib.util.cache_from_source('x.py'))",
            "AttributeError",
            "import importlib.util before",
        ),
        (
            "import sys\nfrom sys import pycache_prefix as old\nsys.pycache_prefix = 'now'\nprint(old is None)",
            "True",
            "from-importing mutable sys.pycache_prefix",
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

    # Imre has no native bytecode, so select the same tag for the peer processes.
    source = "import sys\nfrom importlib.util import cache_from_source\n" + prefix + """sys.implementation.cache_tag = None
a.origin = 'dir/mod.py'
a.has_location = True
print(a.cached is None)
try:
    print(cache_from_source('dir/mod.py'))
except NotImplementedError as exc:
    print(str(exc))
sys.implementation.cache_tag = 'target-314'
path = a.cached
if path is not None:
    print(path)
print(cache_from_source('dir/mod.py'))
"""
    directory = root / "source-cache"
    directory.mkdir()
    ref = subprocess.run([sys.executable, "-I", "-S", "-c", source], check=True, capture_output=True)
    binary = lang.build(source.encode(), directory)
    actual = subprocess.run([binary], check=True, capture_output=True)
    assert (actual.stdout, actual.stderr) == (ref.stdout, ref.stderr), (actual, ref)
