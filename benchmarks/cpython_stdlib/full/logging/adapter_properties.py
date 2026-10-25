"""Exercise the unchanged LoggerAdapter.manager accessors on nested native instances."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
from suite_native import Toolchain

LOGGING = "Lib/logging/__init__.py"
BLOB = "9005f1ef865c9096e6035feb7a53a98f8dade705"
PREFIX = """class Manager:
    number: int

    def __init__(self, number: int) -> None:
        self.number = number

class Target:
    manager: Manager

    def __init__(self, manager: Manager) -> None:
        self.manager = manager
"""


def source(root):
    verify_files(root, {LOGGING: BLOB})
    raw = (root / LOGGING).read_bytes()
    cls, = [n for n in ast.parse(raw).body
            if isinstance(n, ast.ClassDef) and n.name == "LoggerAdapter"]
    get, put = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "manager"]
    assert [ast.unparse(n.decorator_list[0]) for n in (get, put)] == ["property", "manager.setter"]
    accessors = b"".join(raw.splitlines(keepends=True)[get.decorator_list[0].lineno - 1:put.end_lineno])
    entry = PREFIX.encode()
    for name, child in (("Adapter", "Target"), ("Nested", "Adapter")):
        entry += f"""
class {name}:
    logger: {child}

    def __init__(self, logger: {child}) -> None:
        self.logger = logger

""".encode() + accessors
    entry += b"""
original = Manager(3)
replacement = Manager(5)
target = Target(original)
adapter = Adapter(target)
nested = Nested(adapter)
print(nested.manager is original)
nested.manager = replacement
print(target.manager is replacement)
print(adapter.manager is replacement)
print(nested.manager is replacement)
adapter.manager = original
print(nested.manager is original)
"""
    return entry, hashlib.sha256(accessors).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result", "cpython-source"):
        parser.add_argument("--" + arg, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use CPython 3.14.5 with assertions enabled")
    entry, digest = source(args.cpython_source)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        ref = work / "reference.py"
        ref.write_bytes(entry)
        expected = observe([sys.executable, "-I", "-B", ref], work)
        assert expected == ["True"] * 5
        control = HERE.parents[3] / "tests/fixtures/files/logging_adapter_core.py"
        context_expected = observe([sys.executable, "-I", "-B", control], work)
        levels = {}
        public_context = {}
        for level in (0, 3):
            tool = Toolchain(args.imre, args.runtime, args.clang, level)
            actual = observe([tool.build(entry, work)], work)
            if actual != expected:
                raise AssertionError((level, expected, actual))
            levels[str(level)] = actual
            context_actual = observe([tool.build(control.read_bytes(), work)], work)
            if context_actual != context_expected:
                raise AssertionError((level, context_expected, context_actual))
            public_context[str(level)] = {
                "output_lines": len(context_actual),
                "stdout_sha256": hashlib.sha256(("\n".join(context_actual) + "\n").encode()).hexdigest(),
            }
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    report = {
        "cpython_commit": manifest["commit"], "blob": BLOB, "source": LOGGING,
        "method": "LoggerAdapter.manager getter and setter", "sha256": digest,
        "fixture": "Typed native holder and two native adapter layers; no LoggerAdapterTest fixture",
        "logging_occurrences": 0, "levels": levels, "public_context": public_context,
    }
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print(f"LoggerAdapter: five manager and {len(context_expected)} public context checks at O0/O3; zero test IDs")


def observe(command, work):
    child = subprocess.run(command, cwd=work, capture_output=True, check=True, timeout=30)
    if child.stderr:
        raise AssertionError(child.stderr)
    return child.stdout.decode().splitlines()


if __name__ == "__main__":
    main()
