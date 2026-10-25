"""Run unchanged argparse unittest routines against compiled parser declarations."""

import argparse
import ast
from collections import Counter
from functools import cache
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files


class Blocked(Exception):
    """The compiler or the adapter cannot represent this operation."""


def spelling(value):
    # Render values passed by the actual test to the native declaration. Never
    # replace an unsupported converter, callback, or default with host behavior.
    if isinstance(value, type) and value.__module__ == "builtins":
        return value.__name__
    if value is argparse.BooleanOptionalAction:
        return "argparse.BooleanOptionalAction"
    if isinstance(value, (str, int, float, bool, list, tuple, set, range)) or value is None:
        return repr(value)
    raise Blocked(f"cannot emit declaration value {type(value).__module__}.{type(value).__qualname__}")


def arguments(args, kwargs):
    return ", ".join([*(spelling(value) for value in args),
                      *(f"{key}={spelling(value)}" for key, value in kwargs.items())])


def completed(command):
    result = subprocess.run(command, capture_output=True, timeout=60)
    return result.returncode, result.stdout, result.stderr


class Group:
    def __init__(self, declarations, name):
        self.declarations = declarations
        self.name = name

    def add_argument(self, *args, **kwargs):
        self.declarations.append(f"{self.name}.add_argument({arguments(args, kwargs)})")


class Parser(Group):
    def __init__(self, driver, *args, **kwargs):
        super().__init__([f"parser = argparse.ArgumentParser({arguments(args, kwargs)})"], "parser")
        self.driver = driver

    def add_argument_group(self, *args, **kwargs):
        name = f"group{len(self.declarations)}"
        self.declarations.append(f"{name} = parser.add_argument_group({arguments(args, kwargs)})")
        return Group(self.declarations, name)

    def parse_args(self, args=None):
        # The upstream failure wrappers always call parse_args(args), even when
        # their test name ends in sysargs. Observe the actual call each time.
        explicit = args is not None
        arg = "words" if explicit else ""
        decl = "\n    ".join(self.declarations)
        source = (f"import argparse\nimport sys\n\ndef parse({'words: list[str]' if explicit else ''}):\n"
                  f"    {decl}\n    return parser.parse_args({arg})\n\n"
                  f"result = parse({'sys.argv[1:]' if explicit else ''})\n"
                  f"print('RETURN')\nprint(repr(result))\n")
        binary, reason = self.driver.build(source, self.driver.level)
        if reason:
            raise Blocked(reason)
        argv = args if explicit else sys.argv[1:]
        actual = completed([binary, *argv])
        expected = completed([sys.executable, "-B", binary.with_name("cli.py"), *argv])
        if actual != expected:
            raise AssertionError(f"argv={argv!r}: native={actual!r}, CPython={expected!r}")
        self.driver.operations += 1
        code, stdout, stderr = actual
        marker = b"RETURN\n"
        if code or not stdout.startswith(marker):
            from test.test_argparse import ArgumentParserError
            raise ArgumentParserError("SystemExit", stdout.decode(), stderr.decode(), code)
        # Only transport the native namespace. CPython's copy above is used
        # solely as a byte-for-byte process oracle, never to produce the result.
        expr = ast.parse(stdout.removeprefix(marker).decode().strip(), mode="eval").body
        if not isinstance(expr, ast.Call) or not isinstance(expr.func, ast.Name) or expr.func.id != "Namespace":
            raise AssertionError(f"unexpected native namespace: {stdout!r}")
        return types.SimpleNamespace(**{kw.arg: ast.literal_eval(kw.value) for kw in expr.keywords})


class Driver:
    def __init__(self, options, work):
        self.options = options
        self.work = work

    @cache
    def build(self, source, level):
        folder = self.work / hashlib.sha256(source.encode()).hexdigest()
        folder.mkdir(exist_ok=True)
        script = folder / "cli.py"
        script.write_text(source)
        binary = folder / f"native-{level}"
        code, _, error = completed([self.options.imre, "build", script, "--quiet", "-o", binary,
                                    "--runtime", self.options.runtime, "--clang", self.options.clang,
                                    "--opt-level", str(level)])
        if code:
            message = error.decode(errors="replace").strip().replace(str(script), "cli.py")
            return None, message
        return binary, None


class Result(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.problems = []

    def record(self, error):
        kind, value, _ = error
        self.problems.append(("blocked" if issubclass(kind, Blocked) else "failed", str(value)))

    def addError(self, test, err):
        self.record(err)

    def addFailure(self, test, err):
        self.record(err)

    def addSubTest(self, test, subtest, err):
        if err:
            self.record(err)


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
        results = {}
        for cls in dict.fromkeys(type(c) for c in inventory):
            with tempfile.TemporaryDirectory(prefix="imre-argparse-") as work:
                driver = Driver(options, Path(work))
                driver.level = level
                for case in (c for c in inventory if type(c) is cls):
                    name = case.id()
                    if not isinstance(cls, upstream.ParserTesterMetaclass) or not case._testMethodName.startswith(("test_failures_", "test_successes_")):
                        results[name] = ["reference-only", "outside generated parse routine adapter"]
                        continue
                    if cls.parser_class is not upstream.ErrorRaisingArgumentParser:
                        results[name] = ["blocked", f"custom parser class {cls.parser_class.__qualname__}"]
                        continue
                    driver.operations = 0
                    case.parser_class = lambda *a, **kw: Parser(driver, *a, **kw)
                    outcome = Result()
                    case.run(outcome)
                    if outcome.problems:
                        # A differential or assertion failure takes precedence over
                        # a blocked subtest. Never count a partially executed routine.
                        result = next((p for p in outcome.problems if p[0] == "failed"), outcome.problems[0])
                    elif outcome.skipped:
                        result = ["reference-skip", outcome.skipped[0][1]]
                    elif driver.operations:
                        result = ["host-driven-native", driver.operations]
                    else:
                        result = ["reference-only", "no parse_args calls in the upstream routine"]
                    results[name] = result
                driver.build.cache_clear()
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
