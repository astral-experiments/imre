"""Record four-mode hash results against the original default-resource Linux inventory."""
import argparse
import ast
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, reference_cases, roles, write_reports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path, help="directory containing the four native_hashlib.py JSON outputs")
    args = parser.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    measured = None
    modes = []
    for mode in ("ordinary", "reachable"):
        for level in (0, 3):
            label = f"{mode}-O{level}"
            current = json.loads((args.results / f"{label}.json").read_text())
            if current.pop("opt_level") != str(level) or current.pop("reachable_only") != (mode == "reachable"):
                raise ValueError(f"wrong compiler mode for {label}")
            if current["cpython_commit"] != reference["cpython_commit"]:
                raise ValueError("the native and reference evidence must use the same CPython revision")
            if measured is not None and current != measured:
                raise ValueError(f"{label} disagrees with the other hash results")
            measured = current
            modes.append(label)

    (HERE / "hashlib-linux.json").write_text(json.dumps({"modes": modes, **measured}, indent=2) + "\n")
    discovered = [(key, "default Linux regrtest resources") for key, _ in reference_cases(reference)]
    observations = {}
    exceptions = {}
    test_source = "Lib/test/test_hashlib.py"
    original = ast.parse((HERE / "upstream" / test_source).read_text())
    lines = {node.name: min([node.lineno, *(d.lineno for d in node.decorator_list)])
             for node in ast.walk(original) if isinstance(node, ast.FunctionDef)}
    for (key, occurrence), outcome in reference_cases(reference).items():
        if isinstance(outcome, dict) and "skipped" in outcome:
            for level in (0, 3):
                exceptions[key, occurrence, level] = {"kind": "upstream-skip", "reason": outcome["skipped"],
                    "condition": outcome["skipped"], "source": f"{test_source}:{lines[key.rsplit('.', 1)[1]]}",
                    "configuration": "upstream default Linux regrtest resources", "platform": "linux",
                    "fixture_variant": "default Linux regrtest resources"}
            continue
        row = measured["tests"].get(key)
        if row is None or outcome != "pass":
            continue
        if row["status"] == "mixed":
            loc = roles(body="cpython", assertions="cpython", operation="mixed", fixture="mixed")
            detail = ("The original test called the native public constructor or manipulated native hash state. "
                      "CPython exercised its alternate constructors and file operations.")
        elif row["status"] == "pass":
            loc = roles(body="imre", assertions="imre", operation="imre", fixture="mixed")
            detail = ("The original method body and its equality checks ran on Imre. "
                      "The runner supplied the assertion adapter and the private GIL threshold when used.")
        else:
            raise ValueError(f"hash check failed: {key}")
        observations[(key, occurrence)] = {"outcome": "pass", "roles": loc,
                                  "evidence": {"operation": detail, "fixture": "See hashlib-linux.json and native_hashlib.py."}}
    reports = [build_report(reference, level, discovered, observations,
                            exceptions={k: v for k, v in exceptions.items() if k[2] == level})
               for level in (0, 3)]
    write_reports(HERE / "evidence-linux.json", reports, reference)


if __name__ == "__main__":
    main()
