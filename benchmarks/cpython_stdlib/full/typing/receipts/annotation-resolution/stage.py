"""Recreate the measured module roots from vendored CPython 3.14.5 sources."""

import hashlib
import json
from pathlib import Path
import sys


HERE = Path(__file__).resolve().parent
FULL = HERE.parents[2]


def lines(source, first, last):
    return b"".join(source.splitlines(keepends=True)[first - 1 : last])


def stage(destination):
    cases = json.loads((HERE / "results.json").read_bytes())["cases"]
    support = FULL / "shared/upstream/Lib/test"
    for family, case in cases.items():
        source = (FULL / case["upstream"]).read_bytes()
        if hashlib.sha256(source).hexdigest() != case["module_sha256"]:
            raise ValueError(f"Changed upstream module for {family}")
        method = lines(source, *case["method_lines"])
        files = {
            "entry.py": (HERE / f"{family}_case.py").read_bytes(),
            "test/__init__.py": (support / "__init__.py").read_bytes(),
        }
        if family == "annotationlib":
            prefix = b"import unittest\nfrom annotationlib import ForwardRef, Format\nfrom test import support\n\n\n"
            files["test/test_annotationlib.py"] = prefix + lines(source, 1900, 1900) + method
            helper = lines((support / "support/__init__.py").read_bytes(), 3150, 3188)
            files["test/support/__init__.py"] = b"import annotationlib\n\n\n" + helper
        else:
            prefix = b"import typing\nfrom typing import ClassVar, get_type_hints\nfrom unittest import TestCase\n\n\n"
            files["test/test_typing.py"] = prefix + lines(source, 67, 73) + lines(source, 6206, 6206) + method
        digests = {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}
        if digests != case["stage_sha256"]:
            raise ValueError(f"Stage differs from the measured {family} inputs")
        root = destination / family
        root.mkdir()
        for name, data in files.items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)


if __name__ == "__main__":
    stage(Path(sys.argv[1]))
