"""Stage the pinned UtimeTests class and its module fixtures for a native unittest run."""
import argparse
import ast
import json
from pathlib import Path
import shutil
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files

PIN = "5607950ef232dad16d75c0cf53101d9649d89115"


def stage_class(path):
    source = path.read_text()
    lines = source.splitlines(keepends=True)
    output = ["\n"] * len(lines)
    required = {
        "import asyncio", "import os", "import sys", "import time", "import unittest",
        "from test import support", "from test.support import os_helper",
        "create_file", "tearDownModule", "UtimeTests",
    }
    for node in ast.parse(source).body:
        name = getattr(node, "name", None)
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            name = ast.get_source_segment(source, node)
        if name == "setUpModule":
            raise ValueError("inspect the new module setup before staging UtimeTests")
        if name in required:
            first = min([node.lineno] + [d.lineno for d in getattr(node, "decorator_list", [])]) - 1
            output[first:node.end_lineno] = lines[first:node.end_lineno]
            required.remove(name)
    if required:
        raise ValueError(f"missing UtimeTests fixtures: {sorted(required)}")
    path.write_text("".join(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="a new directory for the module root")
    parser.add_argument("--whole-module", action="store_true", help="retain all of test_os.py")
    args = parser.parse_args()
    args.destination.mkdir(parents=True)
    for manifest in (HERE.parent / "shared/UPSTREAM.json", HERE / "os.json"):
        spec = json.loads(manifest.read_text())
        if spec["commit"] != PIN:
            raise ValueError(f"expected CPython 3.14.5 in {manifest}")
        files = {
            name: blob for name, blob in spec["files"].items()
            if name in {"Lib/test/__init__.py", "Lib/test/mapping_tests.py", "Lib/test/test_os.py"}
            or (name.startswith("Lib/test/support/") and name.endswith(".py"))
        }
        upstream = manifest.parent / "upstream"
        verify_files(upstream, files)
        for name in files:
            target = args.destination / name.removeprefix("Lib/")
            target.parent.mkdir(exist_ok=True, parents=True)
            shutil.copyfile(upstream / name, target)
    if not args.whole_module:
        stage_class(args.destination / "test/test_os.py")
    shutil.copyfile(HERE / "entries/utime.py", args.destination / "loader.py")


if __name__ == "__main__":
    main()
