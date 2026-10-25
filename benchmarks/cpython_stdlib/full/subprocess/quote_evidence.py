"""Apply the two unchanged command-line quoting methods to paired native evidence."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports  # noqa: E402

IDS = {
    f"test.test_subprocess.{cls}.test_list2cmdline"
    for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
}
METHOD = "a2dcff9faa9ec44da990768fa1ce096e7ce9f2f43979d9d9d05525939396e731"
ARITIES = [3, 3, 3, 3, 3, 3, 3, 2]


def verify(report):
    if (
        set(report["rows"]) != IDS
        or report["observer_sha256"]
        != hashlib.sha256((HERE / "fixture_quote.c").read_bytes()).hexdigest()
    ):
        raise ValueError("expected both original poll variants and the native observer")
    if set(report["toolchain"]) != {"imre", "runtime", "clang"} or not all(
        re.fullmatch(r"[0-9a-f]{64}", v) for v in report["toolchain"].values()
    ):
        raise ValueError("missing toolchain hashes")
    for name, cases in report["rows"].items():
        [case] = cases
        expected = dict(
            execution="native-body",
            body="Imre",
            assertions="Imre",
            fixture_execution="CPython module/class/instance hooks; native instance in child",
            method_sha256=METHOD,
            arguments_per_original_call=ARITIES,
            native_argument_quotes=sum(ARITIES),
            native_joins=len(ARITIES),
            assertions_rejecting_substituted_result=list(range(1, len(ARITIES) + 1)),
        )
        hashes = [case[k] for k in ("native_program_sha256", "observed_program_sha256")]
        if (
            any(case[k] != v for k, v in expected.items())
            or len(set(hashes)) != 2
            or not all(re.fullmatch(r"[0-9a-f]{64}", h) for h in hashes)
        ):
            raise ValueError(f"unverified operation, assertion or fixture for {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("previous", "quotes", "result"):
        parser.add_argument("--" + key, type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text())
    data = json.loads(args.quotes.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    if (
        data["schema"] != "imre.subprocess.quoting/v1"
        or sorted(r["opt_level"] for r in data["reports"]) != [0, 3]
        or not all(
            re.fullmatch(r"[0-9a-f]{40}", data[k])
            for k in ("imre_revision", "tested_tree")
        )
    ):
        raise ValueError(
            "expected the tested product source and both optimization levels"
        )
    result = []
    for prev in previous:
        level = prev["opt_level"]
        [raw] = [r for r in data["reports"] if r["opt_level"] == level]
        if raw["cpython_commit"] != ref["cpython_commit"]:
            raise ValueError("wrong CPython revision")
        verify(raw)
        rows = prev["records"]
        seen = [(r["id"], r["fixture_variant"]) for r in rows]
        exceptions = {
            (r["id"], r["occurrence"], level): r["exception"]
            for r in rows
            if "exception" in r
        }
        obs = {
            (r["id"], r["occurrence"]): {
                k: r[k] for k in ("outcome", "roles", "reason", "evidence") if k in r
            }
            for r in rows
        }
        for name in IDS:
            obs[name, 1] = dict(
                outcome="pass",
                roles=dict(
                    body="imre", assertions="imre", operation="imre", fixture="cpython"
                ),
                evidence=dict(
                    opt_level=level,
                    imre_revision=data["imre_revision"],
                    tested_tree=data["tested_tree"],
                    source=args.quotes.name,
                    method_sha256=METHOD,
                    operation="Imre quoted all 23 original arguments and joined eight command lines. Substituting the result at any join failed that exact native equality assertion.",
                    fixture="CPython ran the original module, class and instance fixtures. Imre ran the complete unchanged body, all assertions and the quoting.",
                ),
            )
        updated = build_report(ref, level, seen, obs, exceptions=exceptions)
        if any(
            a != b
            for a, b in zip(rows, updated["records"], strict=True)
            if a["id"] not in IDS
        ):
            raise ValueError("unrelated occurrence changed")
        result.append(updated)
    write_reports(args.result, result, ref)


if __name__ == "__main__":
    main()
