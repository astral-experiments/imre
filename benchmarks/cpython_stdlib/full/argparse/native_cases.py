"""Run unchanged argparse unittest routines against compiled parser declarations."""

import argparse
import ast
import builtins
from collections import Counter
from functools import cache, partial
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tokenize
import traceback
import types
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, run_in_fixtures


class Blocked(Exception):
    """The compiler or the adapter cannot represent this operation."""


def spelling(value):
    # Render values passed by the actual test to the native declaration. Never
    # replace an unsupported converter, callback, or default with host behavior.
    if value is argparse.SUPPRESS:
        return "argparse.SUPPRESS"
    if isinstance(value, type) and value.__module__ == "builtins":
        return value.__name__
    if isinstance(value, type) and value.__module__ == "argparse":
        return "argparse." + value.__qualname__
    if isinstance(value, (str, int, float, bool, list, tuple, set, range)) or value is None:
        return repr(value)
    raise Blocked(f"cannot emit declaration value {type(value).__module__}.{type(value).__qualname__}")


def arguments(args, kwargs):
    return ", ".join([*(spelling(value) for value in args),
                      *(f"{key}={spelling(value)}" for key, value in kwargs.items())])


def completed(command):
    result = subprocess.run(command, capture_output=True, timeout=60)
    return result.returncode, result.stdout, result.stderr


def namespace_value(text):
    """Decode native repr; quoted text that resembles a class stays a string."""
    classes = {}
    if "<class " in text:
        parts = []
        tokens = iter(tokenize.generate_tokens(io.StringIO(text).readline))
        for token in tokens:
            if (token.type, token.string) != (tokenize.OP, "<"):
                parts.append((token.type, token.string))
                continue
            tag, literal, end = next(tokens), next(tokens), next(tokens)
            if tag.string != "class" or literal.type != tokenize.STRING or end.string != ">":
                raise AssertionError(f"unexpected native value in {text!r}")
            name = ast.literal_eval(literal.string)
            cls = getattr(builtins, name, None)
            if not isinstance(cls, type) or cls.__module__ != "builtins" or cls.__name__ != name:
                raise Blocked(f"cannot transfer native class {name!r}")
            key = f"_native_class_{len(classes)}"
            classes[key] = cls
            parts.append((tokenize.NAME, key))
        text = tokenize.untokenize(parts)
    expr = ast.parse(text, mode="eval").body
    if not isinstance(expr, ast.Call) or not isinstance(expr.func, ast.Name) or expr.func.id != "Namespace" or expr.args:
        raise AssertionError(f"unexpected native namespace: {text!r}")

    def value(node):
        if isinstance(node, ast.Name) and node.id in classes:
            return classes[node.id]
        if isinstance(node, (ast.List, ast.Tuple)):
            items = [value(item) for item in node.elts]
            return items if isinstance(node, ast.List) else tuple(items)
        return ast.literal_eval(node)

    return types.SimpleNamespace(**{kw.arg: value(kw.value) for kw in expr.keywords})


class Group:
    def __init__(self, declarations, name):
        self.declarations = declarations
        self.name = name

    def add_argument(self, *args, **kwargs):
        self.declarations.append(f"{self.name}.add_argument({arguments(args, kwargs)})")

    def add_mutually_exclusive_group(self, *args, **kwargs):
        name = f"mutex{len(self.declarations)}"
        self.declarations.append(f"{name} = {self.name}.add_mutually_exclusive_group({arguments(args, kwargs)})")
        return Group(self.declarations, name)


class Parser(Group):
    def __init__(self, driver, *args, **kwargs):
        super().__init__([f"parser = argparse.ArgumentParser({arguments(args, kwargs)})"], "parser")
        self.driver = driver

    def add_argument_group(self, *args, **kwargs):
        name = f"group{len(self.declarations)}"
        self.declarations.append(f"{name} = parser.add_argument_group({arguments(args, kwargs)})")
        return Group(self.declarations, name)

    def parse_args(self, args=None):
        return self._parse("parse_args", args)

    def parse_known_args(self, args=None):
        return self._parse("parse_known_args", args)

    def _parse(self, method, args):
        # The upstream failure wrappers always call parse_args(args), even when
        # their test name ends in sysargs. Observe the actual call each time.
        explicit = args is not None
        arg = "words" if explicit else ""
        decl = "\n    ".join(self.declarations)
        result = "result" if method == "parse_args" else "result[0]"
        source = (f"import argparse\nimport sys\n\ndef parse({'words: list[str]' if explicit else ''}):\n"
                  f"    {decl}\n    return parser.{method}({arg})\n\n"
                  f"result = parse({'sys.argv[1:]' if explicit else ''})\n"
                  f"print('RETURN')\nprint(repr({result}))\n")
        if method == "parse_known_args":
            source += "print(repr(result[1]))\n"
        binary, reason = self.driver.build(source)
        if reason:
            self.driver.blocked[self.driver.name].add(reason)
            raise Blocked(reason)
        argv = args if explicit else sys.argv[1:]
        actual = completed([binary, *argv])
        expected = completed([sys.executable, "-B", binary.with_name("case.py"), *argv])
        if actual != expected:
            raise AssertionError(f"argv={argv!r}: native={actual!r}, CPython={expected!r}")
        self.driver.calls[method] += 1
        code, stdout, stderr = actual
        marker = b"RETURN\n"
        if code or not stdout.startswith(marker):
            self.exited(code, stdout, stderr)
        # Only transport the native values. The reference process never supplies
        # any part of the namespace or the extras used by the original assertions.
        lines = stdout.removeprefix(marker).decode().splitlines()
        if len(lines) != (1 if method == "parse_args" else 2):
            raise AssertionError(f"unexpected native result: {stdout!r}")
        namespace = namespace_value(lines[0])
        if method == "parse_args":
            return namespace
        extras = ast.literal_eval(lines[1])
        if not isinstance(extras, list) or not all(isinstance(arg, str) for arg in extras):
            raise AssertionError(f"unexpected native extras: {extras!r}")
        return namespace, extras

    def exited(self, code, stdout, stderr):
        from test.test_argparse import ArgumentParserError
        raise ArgumentParserError("SystemExit", stdout.decode(), stderr.decode(), code)


class Driver:
    def __init__(self, toolchain, work, observations=None):
        self.toolchain = toolchain
        self.work = work
        self.blocked = {}
        self.observations = observations

    @cache
    def build(self, source):
        folder = self.work / hashlib.sha256(source.encode()).hexdigest()
        folder.mkdir(exist_ok=True)
        try:
            return self.toolchain.build(source.encode(), folder), None
        except RuntimeError as error:
            # A setup, linker, or toolchain error is a failure of the run.
            # Only a source diagnostic explains a blocked parser declaration.
            diagnostic = str(error).partition(str(folder / "case.py") + ":")[2]
            if "Imre:" not in diagnostic:
                raise
            return None, "case.py:" + diagnostic.strip()

    def parser_patch(self, case):
        return patch.object(case, "parser_class", partial(Parser, self))

    def execute(self, case, method):
        self.name = case.id()
        self.calls = Counter()
        self.blocked[self.name] = set()
        with self.parser_patch(case):
            try:
                method()
            except Blocked as error:
                self.blocked[self.name].add(str(error))
                raise
        # unittest can still report subtest and teardown failures after this returns.
        return dict(self.calls)

    def outcomes(self, suite, executions, reference):
        evidence, failures, skips = run_in_fixtures(suite, executions)
        results = {}
        for name, entries in evidence.items():
            if not entries:  # The callback raised; the runner reports its failure below.
                continue
            calls, = entries
            operations = sum(calls.values())
            if self.observations is not None:
                self.observations[name] = {
                    "outcome": "pass",
                    "roles": roles(body="cpython", assertions="cpython" if calls else "none",
                                   operation="imre" if calls else "none", fixture="cpython"),
                    "evidence": {"operation": ", ".join("argparse.ArgumentParser." + op for op in calls)
                                 or "upstream input table is empty; no operation ran",
                                 "native_calls": calls, "opt_level": self.toolchain.level,
                                 "fixture": "CPython original method, assertions, and module/class/instance fixtures",
                                 "process_comparison": ("exit status, stdout and stderr against a separate CPython process"
                                                        if calls else "no subprocess needed for the empty input table")},
                }
            results[name] = (["host-driven-native", operations] if operations else
                             ["reference-only", "no parse_args calls in the upstream routine"])
        for name, reason in skips:
            if reference.get(name) != {"skipped": reason}:
                raise ValueError(f"unexpected native skip: {name}: {reason}")
            results[name] = ["reference-skip", reason]
        for occurrence, message in failures:
            # unittest appends a space and the subtest description to its ID.
            name = occurrence.partition(" ")[0]
            if name not in executions:
                raise RuntimeError(f"fixture failed: {occurrence}: {message}")
            # Only a traceback ending in the exact exception we recorded is a
            # blocker. Preserve mismatches and original assertion failures.
            blockers = self.blocked.get(name, ())
            reason = next((r for r in sorted(blockers) if message.endswith(
                "".join(traceback.format_exception_only(Blocked, Blocked(r))))), None)
            if reason is None:
                results[name] = ["failed", message]
            elif results.get(name, [None])[0] != "failed":
                results[name] = ["blocked", reason]
        if set(results) != set(executions):
            raise ValueError("shared fixture runner returned incomplete outcomes")
        if self.observations is not None:
            for name, (status, reason) in results.items():
                if status in {"blocked", "failed", "reference-skip"}:
                    self.observations[name] = {
                        "outcome": {"blocked": "blocked", "failed": "failure", "reference-skip": "skip"}[status],
                        "roles": roles(body="cpython", assertions="unknown", operation="unknown", fixture="cpython"),
                        "reason": reason,
                    }
        return results


def measure(toolchain, module, reference, observations=None):
    from test import test_argparse as upstream
    results, class_cases = {}, {}
    for case in cases(load_suite(module)):
        class_cases.setdefault(type(case), []).append(case)
    for cls, members in class_cases.items():
        with tempfile.TemporaryDirectory(prefix="imre-argparse-") as work:
            driver = Driver(toolchain, Path(work), observations)
            executions = {}
            for case in members:
                name = case.id()
                if not isinstance(cls, upstream.ParserTesterMetaclass) or not case._testMethodName.startswith(("test_failures_", "test_successes_")):
                    results[name] = ["reference-only", "outside generated parse routine adapter"]
                elif cls.parser_class is not upstream.ErrorRaisingArgumentParser:
                    reason = f"custom parser class {cls.parser_class.__qualname__}"
                    results[name] = ["blocked", reason]
                    if observations is not None:
                        observations[name] = {"outcome": "blocked", "reason": reason, "roles": dict.fromkeys(
                            ("body", "assertions", "operation", "fixture"), "unobserved")}
                else:
                    executions[name] = driver.execute
            if executions:
                results.update(driver.outcomes(unittest.TestSuite(members), executions, reference))
            driver.build.cache_clear()
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument("--imre-revision", required=True, help="Git revision used to build both native artifacts")
    options = parser.parse_args()
    options.imre = options.imre.resolve()
    options.runtime = options.runtime.resolve()
    clang = shutil.which(options.clang)
    if clang is None:
        parser.error(f"cannot find clang executable: {options.clang}")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != manifest["tag"]:
        parser.error(f"use a test-enabled CPython {manifest['tag']}")
    import test
    from test import test_argparse as upstream
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2],
                 {**manifest["reference_files"],
                  **{k: v for k, v in manifest["files"].items() if k.startswith("Lib/test/")}})
    reference = json.loads((HERE / "reference-linux.json").read_text())
    inventory = list(cases(load_suite(manifest["module"])))
    if sorted(c.id() for c in inventory) != sorted(reference["reference"]):
        raise ValueError("discovered IDs differ from the pinned reference")
    report = {"module": manifest["module"], "cpython_commit": manifest["commit"],
              "imre_revision": options.imre_revision, "denominator": len(inventory), "summary": {}}
    by_id = {c.id(): [] for c in inventory}
    # tempfile cleanup applies to each class; linked native executables are large.
    for level in (0, 3):
        toolchain = Toolchain(options.imre, options.runtime, Path(clang), level)
        results = measure(toolchain, manifest["module"], reference["reference"])
        counts = dict(sorted(Counter(v[0] for v in results.values()).items()))
        report["summary"][f"-O{level}"] = counts
        for name, result in results.items():
            by_id[name].append(tuple(result))
        print(f"-O{level}:", counts, flush=True)
    outcomes = {}
    for name, levels in sorted(by_id.items()):
        outcomes.setdefault(tuple(levels), []).append(name)
    report["outcomes"] = [{"-O0": low, "-O3": high, "ids": ids}
                          for (low, high), ids in outcomes.items()]
    options.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(c.get("failed", 0) for c in report["summary"].values()))


if __name__ == "__main__":
    sys.exit(main())
