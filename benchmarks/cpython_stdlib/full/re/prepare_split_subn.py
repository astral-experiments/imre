"""Make the assertion controls next to a pinned CPython 3.14.5 test package."""

import argparse
import ast
import hashlib
from pathlib import Path

SOURCE_SHA256 = "879c8b562a5bddb413e73ad6d026a6199785bd08fa1c2c5db1ef831b4e1c47e2"
CHANGES = {
    "test_qualified_re_split": ("['', ':', '', '', 'a:b::c'])", "['', ':', '', '', 'a:b:::c'])"),
    "test_re_split": ("None, '::', 'c'])", "None, ':', 'c'])"),
    "test_re_subn": ("('xxxyxzx', 4)", "('xxxyxzx', 5)"),
}


def prepare(stage: Path) -> None:
    package = stage / "test"
    source = (package / "test_re.py").read_bytes()
    if hashlib.sha256(source).hexdigest() != SOURCE_SHA256:
        raise ValueError("expected Lib/test/test_re.py from CPython v3.14.5")
    lines = source.decode().splitlines(keepends=True)
    cls, = (node for node in ast.parse(source).body
            if isinstance(node, ast.ClassDef) and node.name == "ReTests")
    methods = {node.name: node for node in cls.body if isinstance(node, ast.FunctionDef)}

    # Keep the original physical line numbers for traceback and warning comparisons.
    companion = ["\n"] * len(lines)
    companion[:3] = ["from test import test_re\n", "from test.test_re import S, B\n", "import re\n"]
    companion[cls.lineno - 1] = "class ReTests(test_re.ReTests):\n"
    for name, (expected, changed) in CHANGES.items():
        node = methods[name]
        span = slice(node.lineno - 1, node.end_lineno)
        body = "".join(lines[span])
        if node.decorator_list or body.count(expected) != 1:
            raise ValueError(f"could not identify the pinned expectation in {name}")
        companion[span] = body.replace(expected, changed).splitlines(keepends=True)
    (package / "test_re_split_inverse.py").write_text("".join(companion).rstrip() + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", type=Path, help="working directory containing a copy of Lib/test")
    prepare(parser.parse_args().stage)
