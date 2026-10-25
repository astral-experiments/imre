"""Discover complete CPython suites and record the reference outcome for each ID."""

import argparse
from collections import Counter
import hashlib
import importlib
import json
from pathlib import Path
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


def unique_ids(names):
    counts = Counter(names)
    repeated = [name for name, count in counts.items() if count != 1]
    if not counts or repeated:
        raise ValueError(f"empty inventory or repeated IDs: {repeated}")
    return set(counts)


def reference_results(xml, expected):
    rows = ET.parse(xml).findall(".//testcase")
    seen = unique_ids(row.attrib["name"] for row in rows)
    if seen != expected:
        raise ValueError(f"reference IDs differ: missing={sorted(expected - seen)}, "
                         f"extra={sorted(seen - expected)}")
    results = {}
    for row in rows:
        name = row.attrib["name"]
        if row.get("status") != "run" or row.get("result") != "completed":
            raise ValueError(f"incomplete reference case: {row.attrib}")
        outcome = [child for child in row if child.tag not in {"system-out", "system-err"}]
        if not outcome:
            results[name] = "pass"
        elif len(outcome) == 1 and outcome[0].tag in {"failure", "error", "skipped"}:
            results[name] = {outcome[0].tag: outcome[0].text or outcome[0].get("message", "")}
        else:
            raise ValueError(f"unrecognized reference result: {ET.tostring(row)}")
    return dict(sorted(results.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    version = ".".join(map(str, sys.version_info[:3]))
    if sys.implementation.name != "cpython" or "v" + version != manifest["tag"]:
        parser.error(f"run with test-enabled CPython {manifest['tag']}")
    import test

    module = manifest["module"]
    verify_files(args.manifest.parent / "upstream", manifest["files"])
    # regrtest and its child processes import the interpreter's own Lib/test.
    # Verify those test bytes too. Do not replace package discovery with a glob.
    verify_files(Path(test.__file__).resolve().parents[2],
                 {k: v for k, v in manifest["files"].items() if k.startswith("Lib/test/")})
    ids = unique_ids(case.id() for case in cases(load_suite(module)))
    with tempfile.TemporaryDirectory() as work:
        xml = Path(work) / "reference.xml"
        run = subprocess.run([sys.executable, "-B", "-m", "test", "--single-process",
                              "--tempdir", work, "--randseed", "1", "--junit-xml", xml,
                              module.removeprefix("test.")], cwd=work)
        results = reference_results(xml, ids)
    report = {"module": module, "cpython_commit": manifest["commit"], "platform": sys.platform,
              "denominator": len(ids), "reference": results}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return run.returncode


if __name__ == "__main__":
    sys.exit(main())
