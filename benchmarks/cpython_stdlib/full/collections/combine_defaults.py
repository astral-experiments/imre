"""Add verified native unittest lifecycles to the fixture report for the same compiler."""

import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import classification, validate_pair, write_reports
from suite_inventory import verify_files


def load(path):
    return json.loads(path.read_bytes())


def combine(run):
    reference = load(HERE / "reference-linux.json")
    manifest = load(HERE / "UPSTREAM.json")
    source = load(run / "source.json")
    reports = load(run / "fixture-roles.json")
    validate_pair(reports, reference)
    verify_files(HERE / "upstream", manifest["files"])
    assert source["cpython_commit"] == manifest["commit"]
    for report in reports:
        level = report["opt_level"]
        defaults = load(run / f"native-default-O{level}.json")
        assert defaults["source"] == source and defaults["opt_level"] == level
        assert defaults["reference_denominator"] == report["denominator"]
        for row in report["records"]:
            revision = row.get("evidence", {}).get("compiler_revision")
            assert revision is None or revision == source["compiler_revision"]
            name = row["id"]
            observed = defaults["observations"].get(name)
            if not observed or observed["outcome"] != "pass" or set(observed["roles"].values()) != {"imre"}:
                continue
            module, owner, method = name.rsplit(".", 2)
            relative = "Lib/" + module.replace(".", "/") + ".py"
            data = (HERE / "upstream" / relative).read_bytes()
            cls, = [n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == owner]
            node, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method]
            text = b"".join(data.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
            evidence = observed["evidence"]
            assert evidence["source_blob"] == manifest["files"][relative]
            assert evidence["method_sha256"] == hashlib.sha256(text).hexdigest()
            assert evidence["status"] == 0 and not evidence["stdout"]
            assert evidence["stderr"].endswith("Ran 1 test in <elapsed>s\n\nOK\n")
            row.pop("reason", None)
            row.update(observed)
            row["evidence"] = {**evidence, "compiler_revision": source["compiler_revision"],
                               "opt_level": level, "source": f"{relative}:{node.lineno}"}
            row["classification"] = classification(row)
        report["counts"] = dict(sorted(Counter(r["classification"] for r in report["records"]).items()))
    return reports, reference


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="directory with source, fixture, and native default reports")
    parser.add_argument("--result", required=True, type=Path)
    args = parser.parse_args()
    reports, reference = combine(args.run)
    write_reports(args.result, reports, reference)
