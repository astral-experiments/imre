"""Run each pinned test_ast method that calls literal_eval against a standalone Imre process."""
import argparse
import ast
import builtins
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, verify_files
from suite_native import Toolchain

COMMIT = "5607950ef232dad16d75c0cf53101d9649d89115"
BLOBS = {
    "LICENSE": "20cf39097c68baa17cc566b64e76d34ebf034044",
    "Lib/test/test_ast/__init__.py": "9a89d27ba9f979d0a5ea25d7e10c3c6a5aab7d28",
    "Lib/test/test_ast/test_ast.py": "ef12ce0ce37de8efcc9382a7dc69980e3d699c6b",
    "Lib/test/test_ast/snippets.py": "b76f98901d2ad8b064d9499ed4098ea72c8fe224",
    "Lib/test/test_ast/utils.py": "e7054f3f710f1f0990a4ecc66c210530ab929cc7",
}


def decode(wire):
    kind, value = wire
    if kind == "scalar":
        return value
    if kind == "ellipsis":
        return Ellipsis
    if kind == "bytes":
        return bytes(value)
    if kind == "complex":
        return complex(*value)
    if kind == "dict":
        return {decode(k): decode(v) for k, v in value}
    return {"list": list, "tuple": tuple, "set": set}[kind](map(decode, value))


def constructor(value):
    """Spell a host fixture's AST, including missing fields and source positions."""
    if isinstance(value, ast.AST):
        fields = list(ast.iter_fields(value))
        fields.extend((key, getattr(value, key)) for key in value._attributes if hasattr(value, key))
        name = type(value).__name__
        assert getattr(ast, name) is type(value)
        return f"ast.{name}(" + ", ".join(f"{k}={constructor(v)}" for k, v in fields) + ")"
    if type(value) is list:
        return "[" + ", ".join(map(constructor, value)) + "]"
    if type(value) not in (str, int, bool, float, complex, type(None)):
        raise ValueError(f"unexpected value in upstream AST fixture: {type(value)}")
    return repr(value)


def check(toolchain, root, suite, definitions):
    driver = (HERE / "literal_child.py").read_text()
    entrypoint = "run(sys.argv[1])\n"
    assert driver.endswith(entrypoint)
    entry = root / "string"
    entry.mkdir()
    binary = toolchain.build(driver.encode(), entry)
    results = {}
    for case in cases(suite):
        owner, method = case.id().split(".")[-2:]
        if (owner, method) not in definitions:
            continue
        calls = []

        def native(node_or_string):
            if isinstance(node_or_string, str):
                command = [binary, node_or_string]
            else:
                folder = root / f"{owner}-{method}-{len(calls)}"
                folder.mkdir()
                source = driver.removesuffix(entrypoint) + f"run({constructor(node_or_string)})\n"
                command = [toolchain.build(source.encode(), folder)]
            limit = sys.get_int_max_str_digits()
            proc = subprocess.run(command, capture_output=True, text=True, check=True, timeout=30,
                                  env=dict(os.environ, PYTHONINTMAXSTRDIGITS=str(limit)))
            if proc.stderr:
                raise RuntimeError(proc.stderr)
            response = json.loads(proc.stdout)
            calls.append({"input": type(node_or_string).__name__, "digit_limit": limit,
                          "outcome": response[1] if response[0] == "error" else response[1][0]})
            if response[0] == "error":
                kind, message = response[1:]
                cls = getattr(builtins, kind)
                if not issubclass(cls, Exception):
                    raise RuntimeError(response)
                raise cls(message)
            return decode(response[1])

        with patch.object(ast, "literal_eval", native):
            result = unittest.TestResult()
            unittest.TestSuite([case]).run(result)
        failures = [message for _, message in result.errors + result.failures]
        results[case.id()] = {"outcome": "pass" if result.wasSuccessful() and not result.skipped else "failed",
                              "failures": failures, "skips": [(c.id(), why) for c, why in result.skipped], "calls": calls,
                              "method_sha256": definitions[owner, method],
                              "roles": {"body": "cpython", "assertions": "cpython", "fixture": "cpython",
                                        "operation": "imre"}}
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("compiler", "runtime", "clang", "cpython-source", "output"):
        parser.add_argument("--" + key, required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use pinned test-enabled CPython 3.14.5")
    verify_files(args.cpython_source, BLOBS)
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    from test.test_ast import test_ast
    source = Path(test_ast.__file__).read_bytes()
    assert source == (args.cpython_source / "Lib/test/test_ast/test_ast.py").read_bytes()
    lines = source.splitlines(keepends=True)
    definitions = {}
    for owner in ast.parse(source).body:
        if isinstance(owner, ast.ClassDef):
            for method in owner.body:
                if isinstance(method, ast.FunctionDef) and any(
                    isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name)
                    and n.value.id == "ast" and n.attr == "literal_eval" for n in ast.walk(method)
                ):
                    first = min([method.lineno, *(d.lineno for d in method.decorator_list)])
                    definitions[owner.name, method.name] = hashlib.sha256(
                        b"".join(lines[first - 1:method.end_lineno])).hexdigest()
    report = {"cpython_commit": COMMIT, "source_blobs": BLOBS, "modes": {}}
    for level in (0, 3):
        suite = unittest.defaultTestLoader.loadTestsFromModule(test_ast)
        toolchain = Toolchain(args.compiler, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            results = check(toolchain, Path(tmp), suite, definitions)
        assert {tuple(k.split(".")[-2:]) for k in results} == set(definitions)
        report["modes"][f"O{level}"] = results
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    failures = [f"{level} {k}" for level, rows in report["modes"].items()
                for k, row in rows.items() if row["outcome"] != "pass"]
    if failures:
        raise SystemExit("\n".join(failures))
    print(f"{len(definitions)} unchanged methods at each level")


if __name__ == "__main__":
    main()
