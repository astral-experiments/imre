"""Account for CPython's JSON doctests, inherited methods and repeated package IDs."""

import argparse
from collections import Counter, defaultdict
import doctest
import hashlib
import inspect
import json
from pathlib import Path
import sys
import unittest
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
COMMIT = "5607950ef232dad16d75c0cf53101d9649d89115"
# load_tests also discovers examples from these three *library* modules.
DOC_SOURCES = {
    "Lib/json/__init__.py": "9eaa4f3fbc16793adc061f9f5ee9dd0aab159b20",
    "Lib/json/decoder.py": "92ad635255764098c0feb5aff1a3f4584d92fd90",
    "Lib/json/encoder.py": "5cf6d64f3eade60b65f1324ae394454e97883c45",
}


def blob(path):
    data = path.read_bytes()
    return hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def verify():
    import test.test_json

    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise ValueError("use test-enabled CPython 3.14.5")
    manifest = json.loads((BENCH / "UPSTREAM.json").read_text())
    if manifest["commit"] != COMMIT:
        raise ValueError("expected the CPython 3.14.5 release commit")
    source = Path(test.test_json.__file__).resolve().parents[3]
    suite = "Lib/test/test_json"
    expected = {k: v for k, v in manifest["files"].items()
                if k.startswith(suite + "/") or k == "LICENSE"}
    for root in (source, BENCH / "upstream"):
        files = {p.relative_to(root).as_posix() for p in (root / suite).rglob("*")
                 if p.is_file() and "__pycache__" not in p.parts}
        if files | {"LICENSE"} != expected.keys():
            raise ValueError(f"incomplete JSON package at {root}")
        for name, oid in expected.items():
            if blob(root / name) != oid:
                raise ValueError(f"modified {root / name}")
    for name, oid in DOC_SOURCES.items():
        if blob(source / name) != oid:
            raise ValueError(f"modified doctest source: {name}")


def cases():
    import test.test_json

    def walk(suite):
        for item in suite:
            if isinstance(item, unittest.TestSuite):
                yield from walk(item)
            else:
                yield item

    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(test.test_json)
    if loader.errors:
        raise RuntimeError(loader.errors)
    yield from walk(suite)


def routine(case):
    if isinstance(case, doctest.DocTestCase):
        return case.id()
    name = case._testMethodName
    owner = next(cls for cls in inspect.getmro(type(case)) if name in vars(cls))
    return f"{owner.__module__}.{owner.__qualname__}.{name}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("xml", type=Path, help="CPython regrtest --junit-xml output")
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    verify()
    groups = defaultdict(set)
    seen = Counter()
    for case in cases():
        groups[routine(case)].add(case.id())
        seen[case.id()] += 1
    rows = ET.parse(args.xml).findall(".//testcase")
    if Counter(row.attrib["name"] for row in rows) != seen:
        raise ValueError("reference results differ from package discovery")
    outcomes = defaultdict(list)
    for row in rows:
        if row.get("status") != "run" or row.get("result") != "completed":
            raise ValueError(f"incomplete case: {row.attrib}")
        outcome = [c for c in row if c.tag not in {"system-out", "system-err"}]
        if not outcome:
            result = "pass"
        elif len(outcome) == 1 and outcome[0].tag in {"skipped", "failure", "error"}:
            result = {outcome[0].tag: outcome[0].text or outcome[0].get("message")}
        else:
            raise ValueError(f"unexpected reference outcome: {row.attrib}")
        outcomes[row.attrib["name"]].append(result)
    report = {
        "cpython_commit": COMMIT, "platform": sys.platform,
        "executions": len(rows), "unique_ids": len(seen), "source_routines": len(groups),
        "routines": {k: {i: outcomes[i] for i in sorted(ids)} for k, ids in sorted(groups.items())},
    }
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
