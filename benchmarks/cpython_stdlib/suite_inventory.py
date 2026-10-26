"""Discover complete CPython suites and record the reference outcome for each ID."""

import argparse
from collections import Counter
from contextlib import nullcontext
import hashlib
import importlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET


def verify_files(root, files):
    for name, expected in files.items():
        data = (root / name).read_bytes()
        if hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() != expected:
            raise ValueError(f"upstream blob mismatch: {root / name}")


def discovery_resources(manifest):
    """Use the same resource set as the reference job for package discovery hooks."""
    if not manifest.get("regrtest_discovery", False):
        return nullcontext()
    from test import support
    from unittest.mock import patch
    # Bare unittest's None enables optional resources; default regrtest uses [].
    return patch.object(support, "use_resources", [])


def load_suite(module_name):
    """Honor package load_tests hooks; expose loader failures before counting IDs."""
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(importlib.import_module(module_name))
    if loader.errors:
        raise ValueError("\n".join(loader.errors))
    return suite


def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item


def id_counts(names):
    counts = Counter(names)
    if not counts:
        raise ValueError("empty suite inventory")
    return counts


def subtest_id(name):
    # CPython includes the process address in callable parameters' repr.
    return re.sub(r"(?<= at )0x[0-9a-fA-F]+(?=>)", "0x...", name)


def passing_subtest_skips(suite):
    """Observe each skipped parameter and require a successful sibling in its method."""
    class Result(unittest.TestResult):
        def __init__(self):
            super().__init__()
            self.passing = set()

        def addSubTest(self, test, subtest, err):
            super().addSubTest(test, subtest, err)
            if err is None:
                self.passing.add(test.id())

    result = suite.run(Result())
    if not result.wasSuccessful():
        raise ValueError(f"subtest reference failed: {result.failures!r} {result.errors!r}")
    skips, aliases = [], {}
    for subtest, reason in result.skipped:
        parent = getattr(subtest, "test_case", None)
        if parent is None:  # A whole-method skip keeps its ordinary JUnit outcome.
            continue
        name = parent.id()
        if name not in result.passing:
            raise ValueError(f"{name}: no successful sibling for a skipped subtest")
        params = {key: value.__name__ if callable(value) else value for key, value in subtest.params.items()}
        skips.append([name, params, reason])
        key = subtest_id(subtest.id())
        if aliases.setdefault(key, (name, reason)) != (name, reason):
            raise ValueError(f"ambiguous JUnit subtest ID: {key}")
    return result.testsRun, json.loads(json.dumps(skips)), aliases


def reference_results(xml, expected, subtests=None):
    rows = ET.parse(xml).findall(".//testcase")
    if unnamed := sum(not row.get("name") for row in rows):
        raise ValueError(f"{unnamed} unnamed <testcase> entries cannot be matched to upstream IDs")
    for row in rows:
        if subtests and (observed := subtests.get(subtest_id(row.attrib["name"]))):
            name, reason = observed
            # regrtest's JUnit writer uses the first skipped parameter as the
            # method outcome even when other parameters passed.
            outcomes = [child for child in row if child.tag not in {"system-out", "system-err"}]
            if (len(outcomes) != 1 or outcomes[0].tag != "skipped"
                    or (outcomes[0].text or outcomes[0].get("message", "")) != reason):
                raise ValueError(f"JUnit disagrees with the observed skipped subtest for {name}")
            row.set("name", name)
            row.remove(outcomes[0])
    seen = id_counts(row.attrib["name"] for row in rows)
    if seen != expected:
        raise ValueError(f"reference IDs differ: missing={dict(expected - seen)}, "
                         f"extra={dict(seen - expected)}")
    results = {}
    for row in rows:
        name = row.attrib["name"]
        if row.get("status") != "run" or row.get("result") != "completed":
            raise ValueError(f"incomplete reference case: {row.attrib}")
        outcome = [child for child in row if child.tag not in {"system-out", "system-err"}]
        if not outcome:
            value = "pass"
        elif len(outcome) == 1 and outcome[0].tag in {"failure", "error", "skipped"}:
            value = {outcome[0].tag: outcome[0].text or outcome[0].get("message", "")}
        else:
            raise ValueError(f"unrecognized reference result: {ET.tostring(row)}")
        results.setdefault(name, []).append(value)
    # Keep single occurrences compact without collapsing repeated executions.
    return {name: values[0] if len(values) == 1 else values for name, values in sorted(results.items())}


def verify_interpreter_tests(manifest):
    import test

    verify_files(Path(test.__file__).resolve().parent,
                 {k.removeprefix("Lib/test/"): v for k, v in manifest["files"].items()
                  if k.startswith("Lib/test/")})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--write-bytecode", action="store_true", help="exercise tests that write bytecode; use a disposable CPython prefix")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    version = ".".join(map(str, sys.version_info[:3]))
    if sys.implementation.name != "cpython" or "v" + version != manifest["tag"]:
        parser.error(f"run with test-enabled CPython {manifest['tag']}")
    module = manifest["module"]
    verify_files(args.manifest.parent / "upstream", manifest["files"])
    # regrtest and its child processes import the interpreter's own Lib/test.
    # Verify those test bytes too. Do not replace package discovery with a glob.
    verify_interpreter_tests(manifest)
    with discovery_resources(manifest):
        ids = id_counts(case.id() for case in cases(load_suite(module)))
    skips, aliases = [], {}
    if manifest.get("record_subtest_skips"):
        _, skips, aliases = passing_subtest_skips(load_suite(module))
    with tempfile.TemporaryDirectory() as work:
        xml = Path(work) / "reference.xml"
        flags = [] if args.write_bytecode else ["-B"]
        resources = ["-u", ",".join(manifest["resources"])] if manifest.get("resources") else []
        run = subprocess.run([sys.executable, *flags, "-m", "test", "--single-process", *resources,
                              "--tempdir", work, "--randseed", "1", "--junit-xml", xml,
                              module.removeprefix("test.")], cwd=work)
        results = reference_results(xml, ids, aliases)
    report = {"module": module, "cpython_commit": manifest["commit"], "platform": sys.platform,
              "denominator": sum(ids.values()), "reference": results}
    if manifest.get("record_subtest_skips"):
        report["subtest_skips"] = skips
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return run.returncode


if __name__ == "__main__":
    sys.exit(main())
