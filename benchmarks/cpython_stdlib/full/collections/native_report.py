"""Write the two per-occurrence collections reports using the shared evidence contract."""

import argparse
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, write_reports


UPSTREAM_SKIPS = {
    "test.test_deque.TestSequence.test_free_after_iterating": (
        "The deque-specific override unconditionally calls self.skipTest.",
        "Lib/test/test_deque.py:923"),
    "test.test_userstring.UserStringTest.test_replace_overflow": (
        "sys.maxsize > (1 << 32) or struct.calcsize('P') != 4",
        "Lib/test/string_tests.py:836"),
    "test.test_free_threading.test_collections.TestDeque.test_copy_race": (
        "not test.support.Py_GIL_DISABLED, checked while importing the parent package",
        "Lib/test/test_free_threading/__init__.py:7"),
}
UNOBSERVED = roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved")


def report(reference, routines, docs):
    level = routines["opt_level"]
    if level != docs["opt_level"] or any(r["cpython_commit"] != reference["cpython_commit"] for r in (routines, docs)):
        raise ValueError("the inputs come from different native levels or CPython commits")
    if (not re.fullmatch(r"[0-9a-f]{40}", routines["compiler_revision"])
            or routines["compiler_revision"] != docs["compiler_revision"]):
        raise ValueError("supply the same full Imre revision to both gates")
    original = routines["routines"]
    doc_ids = {name for name, entry in original.items() if entry["result"] == "separate-doctest-gate"}
    if doc_ids != docs["tests"].keys():
        raise ValueError("the doctest gate did not report exactly the discovered DocTestCases")
    discovered, observed, exceptions = [], {}, {}
    for name, entry in original.items():
        key = (name, 1)  # All 659 IDs in this pinned loader are distinct; build_report verifies them.
        variant = entry["variant"]
        discovered.append((name, variant))
        proof = {"compiler_revision": routines["compiler_revision"], "opt_level": level}
        if "source" in entry:
            proof["source"] = entry["source"]
        result = entry["result"]
        if result == "reference-skip":
            condition, source = UPSTREAM_SKIPS[name]
            exceptions[(*key, level)] = {
                "kind": "upstream-skip", "reason": entry["reason"], "condition": condition,
                "source": source, "platform": reference["platform"], "fixture_variant": variant,
                "configuration": "CPython 3.14.5, Linux x86-64, 64-bit pointers, GIL enabled, CPU resource enabled",
            }
            continue
        if result == "separate-doctest-gate":
            doc = docs["tests"][name]
            proof["examples"] = doc["examples"]
            result = doc["status"]
            if result == "native-body/host-output-check":
                proof.update(operation=f"All {doc['examples']} original prompts in {name} executed in Imre",
                             fixture="CPython discovers original doctest and checks output after every prompt")
                observed[key] = {"outcome": "pass", "evidence": proof,
                                 "roles": roles(body="imre", assertions="cpython", operation="imre", fixture="mixed")}
                continue
            reason = doc["diagnostic"]
        elif result == "ran-pending-operation-audit":
            execution, = entry["execution"]
            if "observation" in execution:
                obs = execution["observation"]
                observed[key] = {"outcome": "pass", "roles": obs["roles"], "evidence": proof | obs["evidence"]}
            else:
                observed[key] = {
                    "outcome": "blocked", "reason": "The routine ran; the operation still needs execution attribution.",
                    "roles": roles(body="imre", assertions="unknown", operation="unknown", fixture="mixed"),
                    "evidence": proof | {"method_sha256": execution["method_sha256"]},
                }
            continue
        else:
            reason = entry["reason"]
        if "location" in entry:
            reason = entry["location"] + ": " + reason
        proof["stage"] = result
        if result in {"compile-blocked", "fixture-blocked"}:
            outcome, locations = "blocked", UNOBSERVED
        elif result in {"failed", "build-failed", "fixture-skip"}:
            outcome = "skip" if result == "fixture-skip" else "failure"
            locations = roles(body="unknown", assertions="unknown", operation="unknown", fixture="unknown")
        else:
            raise ValueError(f"unexpected outcome for {name}: {result}")
        observed[key] = {"outcome": outcome, "reason": reason, "roles": locations, "evidence": proof}
    return build_report(reference, level, discovered, observed, exceptions=exceptions)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("routines", "doctests"):
        parser.add_argument("--" + name, type=Path, nargs=2, required=True, metavar=("O0", "O3"))
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports = [report(reference, json.loads(r.read_text()), json.loads(d.read_text()))
               for r, d in zip(args.routines, args.doctests, strict=True)]
    write_reports(args.result, reports, reference)
    for item in reports:
        print(f"-O{item['opt_level']}: {item['denominator']} occurrences, {item['counts']}")


if __name__ == "__main__":
    main()
