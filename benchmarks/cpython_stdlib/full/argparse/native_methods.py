"""Compile complete source-defined argparse methods and run them under their original fixtures."""

import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files
from suite_evidence import ROLES, roles
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture


# Only assign a native operation after inspecting the complete, unchanged body.
# The four parse calls have no explicit assertion. The registration test checks
# both the exception type and its complete message.
OPERATIONS = {
    f"test.test_argparse.TestParseKnownArgs.test_arguments_{sequence}{suffix}":
        ("argparse.ArgumentParser.parse_args", "none")
    for sequence in ("list", "tuple") for suffix in ("", "_positional")
}
OPERATIONS["test.test_argparse.TestBooleanOptionalAction.test_invalid_name"] = (
    "argparse.ArgumentParser.add_argument", "imre")


class Fixtures:
    """Keep argparse's source-defined helpers with the methods that use them."""

    def __init__(self, source):
        self.source = source
        self.lines = source.read_bytes().splitlines(keepends=True)
        module = ast.parse(b"".join(self.lines))
        self.methods = {(cls.name, f.name): f for cls in module.body if isinstance(cls, ast.ClassDef)
                        for f in cls.body if isinstance(f, ast.FunctionDef)}
        self.globals = {}
        for stmt in module.body:
            if isinstance(stmt, ast.Import):
                for name in stmt.names:
                    self.globals[name.asname or name.name] = stmt
            elif isinstance(stmt, ast.ImportFrom):
                for name in stmt.names:
                    self.globals[name.asname or name.name] = stmt
            elif isinstance(stmt, (ast.FunctionDef, ast.ClassDef)) and stmt.name in {
                "NS", "Sig", "StdIOBuffer", "ArgumentParserError", "ErrorRaisingArgumentParser",
                "stderr_to_parser_error",
            }:
                self.globals[stmt.name] = stmt

    def source_bytes(self, node):
        first = min([node.lineno, *(d.lineno for d in getattr(node, "decorator_list", ()))])
        return b"".join(self.lines[first - 1:node.end_lineno])

    @staticmethod
    def owner(cls, name):
        return next((base.__name__ for base in cls.__mro__ if name in base.__dict__), None)

    def fixture(self, cls, node):
        dependencies, helpers, visited = set(), {}, set()
        def collect(current):
            if current in visited:
                return
            visited.add(current)
            for expr in ast.walk(current):
                if isinstance(expr, ast.Name) and expr.id in self.globals:
                    dependency = self.globals[expr.id]
                    dependencies.add(dependency)
                    collect(dependency)
                elif isinstance(expr, ast.Attribute) and isinstance(expr.value, ast.Name) and expr.value.id == "self":
                    owner = self.owner(cls, expr.attr)
                    helper = self.methods.get((owner, expr.attr))
                    if helper is not None and helper is not node and current not in dependencies:
                        helpers[expr.attr] = method_bytes(self.source, owner, expr.attr)
                        collect(helper)
        collect(node)
        prefix = b"".join(self.source_bytes(s) for s in sorted(dependencies, key=lambda s: s.lineno))
        # The shared fixture supplies native unittest assertions. Local argparse
        # helpers retain both their original signature and their complete body.
        return (prefix + b"\n" + testcase_fixture().encode() + b"\n" + b"\n".join(helpers.values())).decode()


def measure(level, toolchain, manifest, reference, source, observations=None):
    fixtures = Fixtures(source)
    classes = {}
    for case in cases(load_suite(manifest["module"])):
        classes.setdefault(type(case), []).append(case)
    results, rejected, entered = {}, {}, set()
    for cls, members in classes.items():
        with tempfile.TemporaryDirectory(prefix="imre-argparse-methods-") as directory:
            work = Path(directory)
            executions = {}
            for case in members:
                name = case.id()
                owner = fixtures.owner(cls, case._testMethodName)
                node = fixtures.methods.get((owner, case._testMethodName))
                if node is None:
                    results[name] = ["generated", "no source definition under this generated name"]
                    continue
                if node.decorator_list:
                    results[name] = ["blocked", "original method decorator: " +
                                     ", ".join(ast.unparse(d) for d in node.decorator_list)]
                    continue
                try:
                    fixture = fixtures.fixture(cls, node)
                except ValueError as exc:
                    results[name] = ["blocked", str(exc)]
                    continue
                key = (owner, case._testMethodName, hashlib.sha256(fixture.encode()).digest())
                if key in rejected:
                    results[name] = rejected[key]
                    continue
                folder = work / case._testMethodName
                folder.mkdir()
                operation, assertions = OPERATIONS.get(name, (None, "imre"))
                try:
                    executions[name] = native_method(
                        toolchain, folder, source, owner, name, fixture,
                        operation=("imre", operation) if operation else None, assertions=assertions)
                except RuntimeError as exc:
                    diagnostic = str(exc).replace(str(folder), "<work>").strip()
                    status = "blocked" if "<work>/case.py:" in diagnostic and "Imre:" in diagnostic else "failed"
                    results[name] = rejected[key] = [status, diagnostic]
            if not executions:
                continue
            entered.update(executions)
            evidence, failures, skips = run_in_fixtures(unittest.TestSuite(members), executions)
            for name, entries in evidence.items():
                if entries:
                    entry, = entries
                    if name in OPERATIONS:
                        if observations is not None:
                            observations[name] = {"outcome": "pass", **entry["observation"]}
                        results[name] = ["native-body-host-fixture", entry["method_sha256"],
                                         OPERATIONS[name][0]]
                    else:
                        results[name] = ["blocked", "native body completed; tested argparse operation needs verification"]
            for name, reason in skips:
                results[name] = ["unexpected-skip", reason]
            for name, reason in failures:
                results[name] = ["failed", reason.replace(directory, "<work>")]
    if observations is not None:
        for name, (status, reason, *_) in results.items():
            if status in {"blocked", "failed", "unexpected-skip"}:
                locations = (roles(body="unknown", assertions="unknown", operation="unknown", fixture="mixed")
                             if name in entered else dict.fromkeys(ROLES, "unobserved"))
                observations[name] = {"outcome": {"blocked": "blocked", "failed": "error", "unexpected-skip": "skip"}[status],
                                      "roles": locations, "reason": reason}
    if set(results) != set(reference):
        raise ValueError("method results differ from pinned discovery")
    print(f"-O{level}: {dict(Counter(value[0] for value in results.values()))}", flush=True)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--imre-revision", required=True)
    parser.add_argument("--result", required=True, type=Path)
    opts = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != manifest["tag"]:
        parser.error(f"run with test-enabled CPython {manifest['tag']}")
    clang = shutil.which(opts.clang)
    if not clang:
        parser.error("clang unavailable")
    import test
    from test import test_argparse as upstream
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], {k: v for k, v in manifest["files"].items() if k.startswith("Lib/test/")})
    reference = json.loads((HERE / "reference-linux.json").read_text())["reference"]
    levels = {}
    for level in (0, 3):
        toolchain = Toolchain(opts.imre.resolve(), opts.runtime.resolve(), Path(clang), level)
        levels[level] = measure(level, toolchain, manifest, reference, Path(upstream.__file__))
    groups = {}
    for name in sorted(reference):
        groups.setdefault(tuple(tuple(levels[lv][name]) for lv in (0, 3)), []).append(name)
    report = {"module": manifest["module"], "cpython_commit": manifest["commit"], "imre_revision": opts.imre_revision,
              "denominator": len(reference), "summary": {f"-O{lv}": dict(Counter(v[0] for v in levels[lv].values())) for lv in (0, 3)},
              "outcomes": [{"-O0": low, "-O3": high, "ids": ids} for (low, high), ids in groups.items()]}
    opts.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(kind in ("failed", "unexpected-skip") for results in levels.values() for kind, *_ in results.values()))


if __name__ == "__main__":
    sys.exit(main())
